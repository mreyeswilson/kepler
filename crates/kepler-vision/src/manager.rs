//! Worker lifecycle: lazy spawn, JSON-lines exchange, wall-clock timeouts with kill, capped
//! output, minimal environment. One worker, one request at a time (`&mut self`).

use crate::allowlist::{check_url, UrlError};
use crate::protocol::{CheckResult, Response, Viewport, MAX_VIEWPORTS, PROTOCOL_VERSION};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Longest worker line accepted (the worker itself caps results at 256 KiB).
pub const MAX_LINE_BYTES: usize = 300 * 1024;
const MAX_JUNK_LINES: u32 = 200;
const MAX_FOREIGN_FRAMES: u32 = 8;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// Default extra time on top of a check's own timeout for the worker to report it.
const GRACE: Duration = Duration::from_secs(5);

/// Environment variables the worker may see. Everything else (tokens, cloud credentials, ...) is
/// withheld; `KEPLER_VISION_*` is forwarded as configuration.
const ENV_PASSTHROUGH: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SYSTEMROOT",
    "windir",
    "ComSpec",
    "HOME",
    "USERPROFILE",
    "TEMP",
    "TMP",
    "TMPDIR",
    "LOCALAPPDATA",
    "APPDATA",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "DISPLAY",
    "PLAYWRIGHT_BROWSERS_PATH",
    "LANG",
    "LC_ALL",
];

#[derive(Debug)]
pub enum VisionError {
    /// `KEPLER_VISION_WORKER` is not set.
    NotConfigured,
    UrlNotAllowed(UrlError),
    BadRequest(String),
    Spawn(String),
    /// The worker did not answer in time and was killed.
    Timeout,
    /// The worker exited or closed its output.
    WorkerExited,
    /// A response line exceeded the cap; the worker was killed.
    TooLarge,
    Protocol(String),
    /// The worker answered with an error frame (stable codes, e.g. `browser_unavailable`).
    Worker {
        code: String,
        message: String,
    },
}

impl std::fmt::Display for VisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(
                f,
                "Vision worker not configured: set KEPLER_VISION_WORKER to workers/vision/dist/src/main.js (build it with `npm run build`)"
            ),
            Self::UrlNotAllowed(e) => write!(f, "URL refused: {e}"),
            Self::BadRequest(m) => write!(f, "invalid request: {m}"),
            Self::Spawn(m) => write!(f, "cannot start the Vision worker: {m}"),
            Self::Timeout => write!(f, "the Vision worker timed out and was stopped"),
            Self::WorkerExited => write!(f, "the Vision worker exited unexpectedly"),
            Self::TooLarge => write!(f, "the Vision worker produced an oversized line and was stopped"),
            Self::Protocol(m) => write!(f, "Vision worker protocol error: {m}"),
            Self::Worker { code, message } => write!(f, "Vision worker error [{code}]: {message}"),
        }
    }
}

impl std::error::Error for VisionError {}

#[derive(Debug, Clone)]
pub struct WorkerCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Extra environment on top of the allowlisted pass-through.
    pub env: Vec<(String, String)>,
}

impl WorkerCommand {
    pub fn new(program: impl Into<OsString>, args: Vec<OsString>) -> Self {
        Self {
            program: program.into(),
            args,
            env: Vec::new(),
        }
    }

    /// `KEPLER_VISION_WORKER=<path>`: a `.js`/`.mjs`/`.cjs` file runs under `KEPLER_VISION_NODE`
    /// (default `node`) with a V8 heap cap; anything else is executed as is.
    pub fn from_env() -> Result<Self, VisionError> {
        let path = std::env::var_os("KEPLER_VISION_WORKER")
            .filter(|p| !p.is_empty())
            .ok_or(VisionError::NotConfigured)?;
        let p = PathBuf::from(&path);
        let is_js = p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "js" | "mjs" | "cjs"));
        if is_js {
            let node = std::env::var_os("KEPLER_VISION_NODE").unwrap_or_else(|| "node".into());
            Ok(Self::new(
                node,
                vec!["--max-old-space-size=384".into(), path],
            ))
        } else {
            Ok(Self::new(path, Vec::new()))
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckOptions {
    /// The worker's own deadline for the whole check (1 s to 120 s).
    pub timeout: Duration,
    /// Where the worker may write screenshots (opt-in; none by default).
    pub screenshot_dir: Option<PathBuf>,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            screenshot_dir: None,
        }
    }
}

enum Line {
    Frame(String),
    TooLarge,
    Eof,
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Line>,
}

pub struct WorkerManager {
    cmd: WorkerCommand,
    allowed_hosts: Vec<String>,
    running: Option<Running>,
    next_id: u64,
    spawns: u32,
    grace: Duration,
}

fn reader(mut out: impl Read + Send + 'static, tx: std::sync::mpsc::SyncSender<Line>) {
    let mut cur: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut junk = 0u32;
    loop {
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => {
                let _ = tx.send(Line::Eof);
                return;
            }
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\n' {
                if cur.first() == Some(&b'{') {
                    if tx
                        .send(Line::Frame(String::from_utf8_lossy(&cur).into_owned()))
                        .is_err()
                    {
                        return;
                    }
                } else if !cur.is_empty() {
                    // Non-protocol output (a stray log line) is ignored, but not without limit.
                    junk += 1;
                    if junk > MAX_JUNK_LINES {
                        let _ = tx.send(Line::TooLarge);
                        return;
                    }
                }
                cur.clear();
            } else {
                cur.push(b);
                if cur.len() > MAX_LINE_BYTES {
                    let _ = tx.send(Line::TooLarge);
                    return;
                }
            }
        }
    }
}

impl WorkerManager {
    /// Does not start anything.
    pub fn new(cmd: WorkerCommand, allowed_hosts: Vec<String>) -> Self {
        Self {
            cmd,
            allowed_hosts,
            running: None,
            next_id: 0,
            spawns: 0,
            grace: GRACE,
        }
    }

    /// Override the grace period added to a check's timeout (mainly for tests).
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// How many times a worker process was started (for tests and diagnostics).
    pub fn spawn_count(&self) -> u32 {
        self.spawns
    }

    fn spawn(&mut self) -> Result<(), VisionError> {
        let mut c = Command::new(&self.cmd.program);
        c.args(&self.cmd.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in std::env::vars_os() {
            let Some(name) = k.to_str() else { continue };
            if ENV_PASSTHROUGH.contains(&name) || name.starts_with("KEPLER_VISION_") {
                c.env(&k, &v);
            }
        }
        for (k, v) in &self.cmd.env {
            c.env(k, v);
        }
        let mut child = c.spawn().map_err(|e| VisionError::Spawn(e.to_string()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(VisionError::Spawn("no stdio".into()));
        };
        let (tx, rx) = sync_channel(16);
        std::thread::spawn(move || reader(stdout, tx));
        self.running = Some(Running { child, stdin, rx });
        self.spawns += 1;
        Ok(())
    }

    fn kill(&mut self) {
        if let Some(mut r) = self.running.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }

    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("r{}", self.next_id)
    }

    /// Send one request and wait for the frame with the same id. Any transport failure kills the
    /// worker (it is restarted lazily by the next call).
    fn exchange(
        &mut self,
        req: &serde_json::Value,
        id: &str,
        wait: Duration,
    ) -> Result<Response, VisionError> {
        let line = serde_json::to_string(req).map_err(|e| VisionError::Protocol(e.to_string()))?;
        let Some(r) = self.running.as_mut() else {
            return Err(VisionError::WorkerExited);
        };
        let wrote = writeln!(r.stdin, "{line}").and_then(|()| r.stdin.flush());
        if wrote.is_err() {
            self.kill();
            return Err(VisionError::WorkerExited);
        }
        let deadline = Instant::now() + wait;
        let mut foreign = 0;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let got = match self.running.as_ref() {
                Some(r) => r.rx.recv_timeout(left),
                None => return Err(VisionError::WorkerExited),
            };
            match got {
                Ok(Line::Frame(s)) => {
                    let resp: Response = serde_json::from_str(&s)
                        .map_err(|e| VisionError::Protocol(format!("bad frame: {e}")))?;
                    if resp.id == id {
                        return Ok(resp);
                    }
                    foreign += 1;
                    if foreign > MAX_FOREIGN_FRAMES {
                        self.kill();
                        return Err(VisionError::Protocol("unexpected frames".into()));
                    }
                }
                Ok(Line::TooLarge) => {
                    self.kill();
                    return Err(VisionError::TooLarge);
                }
                Ok(Line::Eof) | Err(RecvTimeoutError::Disconnected) => {
                    self.kill();
                    return Err(VisionError::WorkerExited);
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.kill();
                    return Err(VisionError::Timeout);
                }
            }
        }
    }

    fn ensure_started(&mut self) -> Result<(), VisionError> {
        if self.running.is_some() {
            return Ok(());
        }
        self.spawn()?;
        let id = self.id();
        let hello = serde_json::json!({"v": PROTOCOL_VERSION, "id": id, "op": "hello"});
        let resp = self.exchange(&hello, &id, STARTUP_TIMEOUT)?;
        if !resp.ok {
            self.kill();
            return Err(worker_error(resp));
        }
        let playwright = resp
            .result
            .as_ref()
            .and_then(|r| r.get("playwright"))
            .and_then(serde_json::Value::as_bool);
        if playwright == Some(false) {
            self.kill();
            return Err(VisionError::Worker {
                code: "playwright_unavailable".into(),
                message: "the `playwright` package is not installed for the worker".into(),
            });
        }
        Ok(())
    }

    /// Start the worker (if needed) and run the handshake, without checking anything.
    pub fn probe(&mut self) -> Result<(), VisionError> {
        self.ensure_started()
    }

    /// Inspect `url` at the given viewports. Validation happens before anything is started.
    pub fn check(
        &mut self,
        url: &str,
        viewports: &[Viewport],
        opts: &CheckOptions,
    ) -> Result<CheckResult, VisionError> {
        check_url(url, &self.allowed_hosts).map_err(VisionError::UrlNotAllowed)?;
        if viewports.is_empty() || viewports.len() > MAX_VIEWPORTS {
            return Err(VisionError::BadRequest(format!(
                "1 to {MAX_VIEWPORTS} viewports"
            )));
        }
        let ms = u64::try_from(opts.timeout.as_millis()).unwrap_or(u64::MAX);
        if !(1_000..=120_000).contains(&ms) {
            return Err(VisionError::BadRequest(
                "timeout must be 1 to 120 seconds".into(),
            ));
        }
        self.ensure_started()?;
        let id = self.id();
        let mut req = serde_json::json!({
            "v": PROTOCOL_VERSION, "id": id, "op": "check", "url": url,
            "viewports": viewports, "timeout_ms": ms,
        });
        if let Some(dir) = &opts.screenshot_dir {
            req["screenshot_dir"] = serde_json::Value::String(dir.display().to_string());
        }
        let resp = self.exchange(&req, &id, opts.timeout + self.grace)?;
        if !resp.ok {
            let fatal = resp
                .error
                .as_ref()
                .is_some_and(|e| matches!(e.code.as_str(), "timeout" | "memory_limit"));
            if fatal {
                self.kill();
            }
            return Err(worker_error(resp));
        }
        let value = resp
            .result
            .ok_or_else(|| VisionError::Protocol("ok frame without result".into()))?;
        serde_json::from_value(value).map_err(|e| VisionError::Protocol(format!("bad result: {e}")))
    }

    /// Ask the worker to stop, then make sure it is gone.
    pub fn shutdown(&mut self) {
        let Some(mut r) = self.running.take() else {
            return;
        };
        let _ = writeln!(
            r.stdin,
            "{}",
            serde_json::json!({"v": PROTOCOL_VERSION, "id": "bye", "op": "shutdown"})
        );
        let _ = r.stdin.flush();
        drop(r.stdin);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match r.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    let _ = r.child.kill();
                    let _ = r.child.wait();
                    return;
                }
            }
        }
    }
}

fn worker_error(resp: Response) -> VisionError {
    match resp.error {
        Some(e) => VisionError::Worker {
            code: e.code,
            message: e.message,
        },
        None => VisionError::Protocol("error frame without error".into()),
    }
}

impl Drop for WorkerManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
