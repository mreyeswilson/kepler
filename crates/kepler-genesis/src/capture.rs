//! Optional PNG capture of the static previews. Best effort: any failure means "HTML only".
//!
//! 1. If the Vision worker is configured or installed (`KEPLER_VISION_WORKER` or
//!    `kepler vision setup`), it is started for the duration of the capture and pointed at a
//!    one-shot `127.0.0.1` server that serves only the design's `preview-N.html` files (the
//!    worker's allowlist only accepts http(s) to localhost).
//! 2. Otherwise a one-shot headless Microsoft Edge / Chrome process takes the screenshot of the
//!    `file://` page and exits (`KEPLER_VISION_BROWSER` may name the browser).
//! 3. Otherwise nothing is captured.
//!
//! Nothing stays resident: the worker is shut down and the browser exits before this returns.

use kepler_vision::{setup, CheckOptions, Viewport, WorkerCommand, WorkerManager};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 1400;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureReport {
    /// `vision-worker`, `edge-headless`, `chrome-headless` or `None` when nothing was available.
    pub method: Option<String>,
    /// One entry per input page: the PNG path when captured.
    pub pngs: Vec<Option<PathBuf>>,
    pub notes: Vec<String>,
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches("//?/").to_string();
    let mut out = String::from("file:///");
    for b in s.trim_start_matches('/').bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Locate a Chromium-family browser without starting anything.
pub fn find_browser() -> Option<(PathBuf, &'static str)> {
    let label = |p: &Path| {
        if p.to_string_lossy().to_ascii_lowercase().contains("edge") {
            "edge-headless"
        } else {
            "chrome-headless"
        }
    };
    if let Some(v) = std::env::var_os("KEPLER_VISION_BROWSER").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(&v);
        if p.is_file() {
            return Some((p.clone(), label(&p)));
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
                candidates.push(base.join("Microsoft/Edge/Application/msedge.exe"));
                candidates.push(base.join("Google/Chrome/Application/chrome.exe"));
            }
        }
    } else {
        for p in [
            "/usr/bin/microsoft-edge",
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ] {
            candidates.push(PathBuf::from(p));
        }
    }
    candidates.into_iter().find(|p| p.is_file()).map(|p| {
        let l = label(&p);
        (p, l)
    })
}

fn kill(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn browser_capture(exe: &Path, html: &Path, png: &Path, scratch: &Path) -> Result<(), String> {
    let profile = scratch.join("browser-profile");
    let _ = std::fs::create_dir_all(&profile);
    let _ = std::fs::remove_file(png);
    let mut child = Command::new(exe)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={WIDTH},{HEIGHT}"))
        .arg(format!("--screenshot={}", png.display()))
        .arg(file_url(html))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the browser: {}", e.kind()))?;
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    let mut written_at: Option<Instant> = None;
    let done = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if Instant::now() < deadline => {
                // Headless Chromium can linger after writing the file: once the PNG has existed
                // for a second, stop waiting for the process.
                if png.metadata().is_ok_and(|m| m.len() > 0) {
                    let t = *written_at.get_or_insert_with(Instant::now);
                    if t.elapsed() > Duration::from_secs(1) {
                        break false;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => break false,
        }
    };
    if !done {
        kill(&mut child);
    }
    let _ = std::fs::remove_dir_all(&profile);
    match png.metadata() {
        Ok(m) if m.len() > 100 => Ok(()),
        _ => Err("the browser did not produce a screenshot".into()),
    }
}

// ---------------------------------------------------------------- Vision worker route

fn worker_command(home: &Path) -> Option<WorkerCommand> {
    match WorkerCommand::from_env() {
        Ok(c) => Some(c),
        Err(_) => {
            let dir = home.join("vision-worker");
            setup::is_installed(&dir).then(|| WorkerCommand::from_path(setup::entry_point(&dir)))
        }
    }
}

fn serve_previews(dir: PathBuf, stop: Arc<AtomicBool>, listener: TcpListener) {
    let _ = listener.set_nonblocking(true);
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut s, _)) => handle(&mut s, &dir),
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn handle(s: &mut TcpStream, dir: &Path) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = [0u8; 2048];
    let n = s.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = req
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("");
    let name = path.trim_start_matches('/');
    let ok = name.starts_with("preview-")
        && name.ends_with(".html")
        && name.len() < 24
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'));
    let body = if ok {
        std::fs::read(dir.join(name)).ok()
    } else {
        None
    };
    let _ = match body {
        Some(b) => {
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                b.len()
            );
            s.write_all(head.as_bytes()).and_then(|_| s.write_all(&b))
        }
        None => {
            s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        }
    };
}

fn worker_capture(
    cmd: WorkerCommand,
    dir: &Path,
    htmls: &[PathBuf],
    pngs: &[PathBuf],
    scratch: &Path,
) -> Vec<Result<(), String>> {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        return htmls
            .iter()
            .map(|_| Err("cannot bind a local port".into()))
            .collect();
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let stop = Arc::new(AtomicBool::new(false));
    let server = {
        let (dir, stop) = (dir.to_path_buf(), stop.clone());
        std::thread::spawn(move || serve_previews(dir, stop, listener))
    };
    let mut mgr = WorkerManager::new(cmd, Vec::new());
    let mut out = Vec::new();
    for (html, png) in htmls.iter().zip(pngs) {
        let name = html
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let url = format!("http://127.0.0.1:{port}/{name}");
        let shots = scratch.join("worker-shots");
        let _ = std::fs::create_dir_all(&shots);
        let r = mgr
            .check(
                &url,
                &[Viewport::new("desktop", WIDTH, 900)],
                &CheckOptions {
                    timeout: CAPTURE_TIMEOUT,
                    screenshot_dir: Some(shots),
                },
            )
            .map_err(|e| e.to_string())
            .and_then(|res| {
                res.viewports
                    .first()
                    .and_then(|v| v.screenshot.clone())
                    .ok_or_else(|| "the worker returned no screenshot".to_string())
            })
            .and_then(|src| {
                std::fs::copy(src, png)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            });
        out.push(r);
    }
    mgr.shutdown();
    stop.store(true, Ordering::Relaxed);
    let _ = server.join();
    let _ = std::fs::remove_dir_all(scratch.join("worker-shots"));
    out
}

/// Capture `htmls[i]` into `pngs[i]`. `home` is the Kepler data directory, `scratch` a private
/// temporary directory the caller removes.
pub fn capture(
    home: &Path,
    design_dir: &Path,
    htmls: &[PathBuf],
    pngs: &[PathBuf],
    scratch: &Path,
) -> CaptureReport {
    let mut report = CaptureReport {
        pngs: vec![None; htmls.len()],
        ..Default::default()
    };
    let _ = std::fs::create_dir_all(scratch);
    let mut pending: Vec<usize> = (0..htmls.len()).collect();
    if let Some(cmd) = worker_command(home) {
        let results = worker_capture(cmd, design_dir, htmls, pngs, scratch);
        for (i, r) in results.into_iter().enumerate() {
            match r {
                Ok(()) => {
                    report.pngs[i] = Some(pngs[i].clone());
                    report.method.get_or_insert_with(|| "vision-worker".into());
                }
                Err(e) => report.notes.push(format!("vision worker: {e}")),
            }
        }
        pending.retain(|i| report.pngs[*i].is_none());
    }
    if !pending.is_empty() {
        match find_browser() {
            Some((exe, label)) => {
                for i in pending {
                    match browser_capture(&exe, &htmls[i], &pngs[i], scratch) {
                        Ok(()) => {
                            report.pngs[i] = Some(pngs[i].clone());
                            report.method.get_or_insert_with(|| label.into());
                        }
                        Err(e) => report.notes.push(format!("{label}: {e}")),
                    }
                }
            }
            None => report
                .notes
                .push("no Vision worker and no Edge/Chrome found: previews are HTML only".into()),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_are_percent_encoded() {
        let u = file_url(Path::new(r"C:\Users\Some One\kepler\preview-1.html"));
        assert_eq!(u, "file:///C:/Users/Some%20One/kepler/preview-1.html");
        assert!(!file_url(Path::new("/tmp/a b/x\"y.html")).contains('"'));
    }

    #[test]
    fn preview_server_only_serves_preview_files() {
        let dir = crate::inspect::tests::temp_dir("srv");
        std::fs::write(dir.join("preview-1.html"), "<p>ok</p>").unwrap();
        std::fs::write(dir.join("spec.json"), "secret").unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let t = {
            let (d, s) = (dir.clone(), stop.clone());
            std::thread::spawn(move || serve_previews(d, s, l))
        };
        let get = |p: &str| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(format!("GET /{p} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .unwrap();
            let mut r = String::new();
            let _ = s.read_to_string(&mut r);
            r
        };
        assert!(
            get("preview-1.html").contains("200 OK") && get("preview-1.html").contains("<p>ok</p>")
        );
        for bad in [
            "spec.json",
            "../spec.json",
            "preview-9.html",
            "preview-1.html/../spec.json",
        ] {
            let r = get(bad);
            assert!(r.contains("404") && !r.contains("secret"), "{bad}: {r}");
        }
        stop.store(true, Ordering::Relaxed);
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
