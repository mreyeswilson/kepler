//! Optional model-assisted intent check for Forge (ADR-0026), adapter side.
//!
//! Runs only inside `kepler hook claude-code UserPromptSubmit`, only when the deterministic
//! rules are *doubtful*, and only for the prompt text (redacted and truncated first). It spawns
//! the user's own `claude -p` (argv, never a shell, no tools, no session persistence, hooks
//! disabled, a guard variable so the child can never re-enter Kepler) with its own deadline and
//! process-tree kill. Any failure means "rules alone": it never blocks or errors the prompt.
//!
//! State shared between short-lived hook processes (circuit breaker, latency stats, a small
//! cache keyed by a hash of the normalized prompt, never the prompt itself) lives in one JSON
//! file in the Kepler data directory. It is advisory: unreadable or corrupt state is ignored.

use kepler_forge::model::{
    note, parse_verdict, prepare_prompt, user_message, ClassifyError, IntentClassifier,
    ModelVerdict, PROVENANCE, SYSTEM_PROMPT,
};
use kepler_policy::Derived;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Environment guard set on the child: `kepler hook` exits immediately when it sees it.
pub const CHILD_GUARD: &str = "KEPLER_FORGE_CHILD";
pub const DEFAULT_MODEL: &str = "haiku";
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_BREAKER_MINUTES: u64 = 10;
/// Consecutive failures that open the circuit breaker.
pub const BREAKER_THRESHOLD: u64 = 2;
const MAX_CACHE: usize = 64;
const MAX_LATENCIES: usize = 20;
const MAX_CLI_OUTPUT: usize = 64 * 1024;
const STATE_FILE: &str = "forge-model.json";

fn env_off(key: &str) -> bool {
    std::env::var(key)
        .map(|v| {
            matches!(
                v.trim().to_lowercase().as_str(),
                "off" | "0" | "false" | "no" | "none"
            )
        })
        .unwrap_or(false)
}

/// Only a plain alias or model id (no leading dash, no spaces) may reach argv.
fn sanitize_model(raw: &str) -> String {
    let m = raw.trim();
    if !m.is_empty()
        && m.len() <= 64
        && !m.starts_with('-')
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']'))
    {
        m.to_string()
    } else {
        DEFAULT_MODEL.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub enabled: bool,
    /// Why it is disabled (for `doctor`), when it is.
    pub disabled_by: Option<&'static str>,
    pub bin: OsString,
    pub model: String,
    pub timeout: Duration,
    pub breaker: Duration,
}

impl ModelConfig {
    pub fn from_env() -> Self {
        let model_var = std::env::var("KEPLER_FORGE_MODEL").unwrap_or_default();
        let disabled_by = if std::env::var_os(CHILD_GUARD).is_some() {
            Some("child_guard")
        } else if env_off("KEPLER_FORGE") {
            Some("KEPLER_FORGE=off")
        } else if env_off("KEPLER_FORGE_MODEL") {
            Some("KEPLER_FORGE_MODEL=off")
        } else {
            None
        };
        let model = sanitize_model(&model_var);
        let ms = |k: &str, d: u64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(d)
        };
        Self {
            enabled: disabled_by.is_none(),
            disabled_by,
            bin: std::env::var_os("KEPLER_CLAUDE_BIN")
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "claude".into()),
            model,
            timeout: Duration::from_millis(
                ms("KEPLER_FORGE_MODEL_TIMEOUT_MS", DEFAULT_TIMEOUT_MS).clamp(100, 60_000),
            ),
            breaker: Duration::from_secs(
                ms("KEPLER_FORGE_BREAKER_MINUTES", DEFAULT_BREAKER_MINUTES).clamp(1, 24 * 60) * 60,
            ),
        }
    }
}

/// `claude -p` as an [`IntentClassifier`].
pub struct ClaudeCliClassifier<'a> {
    pub cfg: &'a ModelConfig,
    /// Usage reported by the CLI for the last successful call: (input tokens, output tokens, USD).
    pub usage: std::cell::Cell<Option<(u64, u64, f64)>>,
}

impl<'a> ClaudeCliClassifier<'a> {
    pub fn new(cfg: &'a ModelConfig) -> Self {
        Self {
            cfg,
            usage: std::cell::Cell::new(None),
        }
    }

    /// The exact argv (after the program), exposed for tests and documentation.
    pub fn argv(model: &str) -> Vec<String> {
        [
            "-p",
            "--model",
            model,
            "--output-format",
            "json",
            "--tools",
            "",
            "--effort",
            "low",
            "--no-session-persistence",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--setting-sources",
            "",
            "--settings",
            "{\"disableAllHooks\":true}",
            "--system-prompt",
            SYSTEM_PROMPT,
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
    }
}

impl IntentClassifier for ClaudeCliClassifier<'_> {
    fn classify(&self, prompt: &str) -> Result<ModelVerdict, ClassifyError> {
        let raw = run_child(self.cfg, &user_message(prompt))?;
        let v: Value =
            serde_json::from_slice(&raw).map_err(|_| ClassifyError::Failed("cli_output"))?;
        if v.get("is_error").and_then(Value::as_bool) == Some(true) {
            return Err(ClassifyError::Failed("cli_error"));
        }
        let text = v
            .get("result")
            .and_then(Value::as_str)
            .ok_or(ClassifyError::Failed("cli_output"))?;
        let verdict = parse_verdict(text)?;
        if let Some(u) = v.get("usage") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let tin =
                n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
            let cost = v
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            self.usage.set(Some((tin, n("output_tokens"), cost)));
        }
        Ok(verdict)
    }
}

fn kill_tree(child: &mut std::process::Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = pid;
    let _ = child.kill();
    let _ = child.wait();
}

fn run_child(cfg: &ModelConfig, stdin_text: &str) -> Result<Vec<u8>, ClassifyError> {
    let mut cmd = Command::new(&cfg.bin);
    cmd.args(ClaudeCliClassifier::argv(&cfg.model))
        .env(CHILD_GUARD, "1")
        .env("KEPLER_FORGE", "off")
        // Skip model-config/telemetry round trips: measured ~2.5 s faster per call (ADR-0026).
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Neutral working directory: the project's CLAUDE.md is irrelevant to a classifier.
    if let Ok(d) = kepler_storage::dirs::data_dir() {
        let sandbox = d.join("forge-sandbox");
        if std::fs::create_dir_all(&sandbox).is_ok() {
            cmd.current_dir(sandbox);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ClassifyError::Unavailable("binary_not_found")
        } else {
            ClassifyError::Failed("spawn")
        }
    })?;
    if let Some(mut stdin) = child.stdin.take() {
        let data = stdin_text.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.take(MAX_CLI_OUTPUT as u64 + 1).read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    let started = Instant::now();
    let Ok(buf) = rx.recv_timeout(cfg.timeout) else {
        kill_tree(&mut child);
        return Err(ClassifyError::Timeout);
    };
    // stdout closed: the process is finishing. Give it a moment, then end the whole tree.
    let remaining = cfg
        .timeout
        .saturating_sub(started.elapsed())
        .max(Duration::from_millis(300));
    let deadline = Instant::now() + remaining;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => break None,
        }
    };
    let Some(status) = status else {
        kill_tree(&mut child);
        return Err(ClassifyError::Timeout);
    };
    if buf.len() > MAX_CLI_OUTPUT {
        return Err(ClassifyError::Failed("output_too_large"));
    }
    if !status.success() {
        return Err(ClassifyError::Failed("exit_status"));
    }
    Ok(buf)
}

// ---------------------------------------------------------------- shared state

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Cache key: hash of the lower-cased, whitespace-collapsed prepared prompt.
pub fn cache_key(prepared: &str) -> String {
    let norm = prepared
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!("{:016x}", fnv1a(norm.as_bytes()))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub fails: u64,
    pub disabled_until: u64,
    pub last_error: String,
    pub calls: u64,
    pub ok: u64,
    pub cache_hits: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    pub lat_ms: Vec<u64>,
    /// (key, canonical verdict JSON), oldest first.
    pub cache: Vec<(String, String)>,
}

impl State {
    fn from_value(v: &Value) -> Self {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Self {
            fails: n("fails"),
            disabled_until: n("disabled_until"),
            last_error: v
                .get("last_error")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(48)
                .collect(),
            calls: n("calls"),
            ok: n("ok"),
            cache_hits: n("cache_hits"),
            tokens_in: n("tokens_in"),
            tokens_out: n("tokens_out"),
            cost_usd: v
                .get("cost_usd")
                .and_then(Value::as_f64)
                .filter(|c| c.is_finite())
                .unwrap_or(0.0),
            lat_ms: v
                .get("lat_ms")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_u64)
                        .rev()
                        .take(MAX_LATENCIES)
                        .collect::<Vec<_>>()
                })
                .map(|mut a| {
                    a.reverse();
                    a
                })
                .unwrap_or_default(),
            cache: v
                .get("cache")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            Some((
                                e.get("h")?.as_str()?.to_string(),
                                e.get("v")?.as_str()?.to_string(),
                            ))
                        })
                        .rev()
                        .take(MAX_CACHE)
                        .collect::<Vec<_>>()
                })
                .map(|mut a| {
                    a.reverse();
                    a
                })
                .unwrap_or_default(),
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "v": 1, "fails": self.fails, "disabled_until": self.disabled_until,
            "last_error": self.last_error, "calls": self.calls, "ok": self.ok,
            "cache_hits": self.cache_hits, "tokens_in": self.tokens_in,
            "tokens_out": self.tokens_out, "cost_usd": self.cost_usd, "lat_ms": self.lat_ms,
            "cache": self.cache.iter().map(|(h, v)| json!({"h": h, "v": v})).collect::<Vec<_>>(),
        })
    }

    pub fn breaker_open(&self, now: u64) -> bool {
        now < self.disabled_until
    }

    pub fn avg_latency_ms(&self) -> Option<u64> {
        (!self.lat_ms.is_empty())
            .then(|| self.lat_ms.iter().sum::<u64>() / self.lat_ms.len() as u64)
    }

    fn record_success(&mut self, ms: u64) {
        self.calls += 1;
        self.ok += 1;
        self.fails = 0;
        self.last_error.clear();
        self.lat_ms.push(ms);
        if self.lat_ms.len() > MAX_LATENCIES {
            self.lat_ms.remove(0);
        }
    }

    fn record_failure(&mut self, code: &str, now: u64, breaker: Duration) {
        self.calls += 1;
        self.fails += 1;
        self.last_error = code.chars().take(48).collect();
        if self.fails >= BREAKER_THRESHOLD {
            self.disabled_until = now + breaker.as_secs();
            self.fails = 0;
        }
    }

    fn cache_get(&self, key: &str) -> Option<ModelVerdict> {
        let (_, v) = self.cache.iter().find(|(h, _)| h == key)?;
        // Re-validated: the state file is not trusted either.
        parse_verdict(v).ok()
    }

    fn cache_put(&mut self, key: String, v: &ModelVerdict) {
        let canon = json!({
            "intent": v.intent.as_str(), "is_task": v.is_task,
            "confidence": v.confidence, "missing": v.missing,
        })
        .to_string();
        self.cache.retain(|(h, _)| *h != key);
        self.cache.push((key, canon));
        while self.cache.len() > MAX_CACHE {
            self.cache.remove(0);
        }
    }
}

pub struct Store {
    path: Option<PathBuf>,
}

impl Store {
    pub fn open() -> Self {
        Self {
            path: kepler_storage::dirs::data_dir()
                .ok()
                .map(|d| d.join(STATE_FILE)),
        }
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    pub fn load(&self) -> State {
        let Some(p) = &self.path else {
            return State::default();
        };
        std::fs::read(p)
            .ok()
            .filter(|b| b.len() <= 256 * 1024)
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .map(|v| State::from_value(&v))
            .unwrap_or_default()
    }

    pub fn save(&self, s: &State) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = p.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, s.to_value().to_string()).is_ok()
            && std::fs::rename(&tmp, p).is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

// ---------------------------------------------------------------- orchestration

/// Why the model was or was not used (stable codes; never user text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Consulted {
    /// A labeled note was produced (from the cache when `cached`).
    Note { cached: bool },
    /// The model answered but it adds nothing (not a task / low confidence).
    NoNote { cached: bool },
    /// The model was not used.
    Skipped(&'static str),
}

/// Ask the classifier about a doubtful prompt, honoring disable switches, the breaker and the
/// cache. Returns the derived note, if any. `usage` is read from the classifier by the caller.
pub fn consult(
    cfg: &ModelConfig,
    classifier: &dyn IntentClassifier,
    store: &Store,
    text: &str,
    now: u64,
) -> (Option<Derived>, Consulted) {
    if !cfg.enabled {
        return (None, Consulted::Skipped("disabled"));
    }
    let mut st = store.load();
    if st.breaker_open(now) {
        return (None, Consulted::Skipped("breaker_open"));
    }
    let prepared = prepare_prompt(text);
    let key = cache_key(&prepared);
    let to_derived = |v: &ModelVerdict| note(v).map(|n| Derived::new("forge", n, PROVENANCE));
    if let Some(v) = st.cache_get(&key) {
        st.cache_hits += 1;
        store.save(&st);
        let d = to_derived(&v);
        let c = if d.is_some() {
            Consulted::Note { cached: true }
        } else {
            Consulted::NoNote { cached: true }
        };
        return (d, c);
    }
    let t0 = Instant::now();
    let r = classifier.classify(&prepared);
    let ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
    match r {
        Ok(v) => {
            st.record_success(ms);
            st.cache_put(key, &v);
            store.save(&st);
            let d = to_derived(&v);
            let c = if d.is_some() {
                Consulted::Note { cached: false }
            } else {
                Consulted::NoNote { cached: false }
            };
            (d, c)
        }
        Err(ClassifyError::Unavailable(code)) => (None, Consulted::Skipped(code)),
        Err(e) => {
            st.record_failure(e.code(), now, cfg.breaker);
            store.save(&st);
            (None, Consulted::Skipped(e.code()))
        }
    }
}

/// Fold CLI-reported usage into the state (best effort).
pub fn add_usage(store: &Store, usage: (u64, u64, f64)) {
    let mut st = store.load();
    st.tokens_in += usage.0;
    st.tokens_out += usage.1;
    st.cost_usd += usage.2;
    store.save(&st);
}

// ---------------------------------------------------------------- doctor

fn binary_found(bin: &OsString) -> bool {
    let p = std::path::Path::new(bin);
    if p.components().count() > 1 {
        return p.is_file();
    }
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd"]
    } else {
        &[""]
    };
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|d| {
            exts.iter()
                .any(|e| d.join(format!("{}{e}", bin.to_string_lossy())).is_file())
        })
    })
}

pub fn doctor_value() -> Value {
    let cfg = ModelConfig::from_env();
    let st = Store::open().load();
    let now = now_secs();
    json!({
        "enabled": cfg.enabled,
        "disabled_by": cfg.disabled_by,
        "model": cfg.model,
        "binary": cfg.bin.to_string_lossy(),
        "binary_found": binary_found(&cfg.bin),
        "timeout_ms": u64::try_from(cfg.timeout.as_millis()).unwrap_or(u64::MAX),
        "breaker": {
            "open": st.breaker_open(now),
            "retry_in_secs": st.disabled_until.saturating_sub(now),
            "consecutive_failures": st.fails,
            "last_error": st.last_error,
        },
        "calls": st.calls, "ok": st.ok, "cache_hits": st.cache_hits,
        "avg_latency_ms": st.avg_latency_ms(),
        "tokens_in": st.tokens_in, "tokens_out": st.tokens_out, "cost_usd": st.cost_usd,
        "sends_to_anthropic": "only the prompt (redacted, <= 2000 chars) of prompts the rules cannot classify; opt out: KEPLER_FORGE_MODEL=off",
    })
}

pub fn doctor_text() -> String {
    let v = doctor_value();
    let b = |k: &str| v[k].as_bool().unwrap_or(false);
    let mut s = String::from("\nForge model assist (optional, ADR-0026)\n");
    if b("enabled") {
        s.push_str(&format!(
            "  status: enabled (model {})\n",
            v["model"].as_str().unwrap_or("?")
        ));
    } else {
        s.push_str(&format!(
            "  status: disabled ({})\n",
            v["disabled_by"].as_str().unwrap_or("?")
        ));
    }
    s.push_str(&format!(
        "  claude binary: {} ({})\n",
        if b("binary_found") {
            "found"
        } else {
            "NOT found; rules only"
        },
        v["binary"].as_str().unwrap_or("?")
    ));
    let br = &v["breaker"];
    if br["open"].as_bool().unwrap_or(false) {
        s.push_str(&format!(
            "  circuit breaker: OPEN, rules only for ~{}s more (last error: {})\n",
            br["retry_in_secs"],
            br["last_error"].as_str().unwrap_or("?")
        ));
    } else {
        s.push_str(&format!(
            "  circuit breaker: closed ({} consecutive failures)\n",
            br["consecutive_failures"]
        ));
    }
    s.push_str(&format!(
        "  calls: {} ({} ok, {} from cache); avg latency: {}; tokens in/out: {}/{}; cost: ${:.4}\n",
        v["calls"],
        v["ok"],
        v["cache_hits"],
        v["avg_latency_ms"]
            .as_u64()
            .map_or("n/a".to_string(), |m| format!("{m} ms")),
        v["tokens_in"],
        v["tokens_out"],
        v["cost_usd"].as_f64().unwrap_or(0.0)
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        answers: RefCell<Vec<Result<ModelVerdict, ClassifyError>>>,
        calls: RefCell<u32>,
    }

    impl Fake {
        fn new(a: Vec<Result<ModelVerdict, ClassifyError>>) -> Self {
            Self {
                answers: RefCell::new(a),
                calls: RefCell::new(0),
            }
        }
    }

    impl IntentClassifier for Fake {
        fn classify(&self, _: &str) -> Result<ModelVerdict, ClassifyError> {
            *self.calls.borrow_mut() += 1;
            let mut a = self.answers.borrow_mut();
            if a.is_empty() {
                Err(ClassifyError::Timeout)
            } else {
                a.remove(0)
            }
        }
    }

    fn task() -> ModelVerdict {
        parse_verdict(r#"{"intent":"ops","is_task":true,"confidence":0.9,"missing":["target"]}"#)
            .unwrap()
    }

    fn cfg() -> ModelConfig {
        ModelConfig {
            enabled: true,
            disabled_by: None,
            bin: "claude".into(),
            model: "haiku".into(),
            timeout: Duration::from_millis(500),
            breaker: Duration::from_secs(600),
        }
    }

    fn store(name: &str) -> Store {
        let p = std::env::temp_dir().join(format!("kepler-fm-{}-{name}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Store::at(p)
    }

    #[test]
    fn note_is_model_inferred_and_cached_by_normalized_hash() {
        let s = store("cache");
        let f = Fake::new(vec![Ok(task())]);
        let (d, c) = consult(&cfg(), &f, &s, "Elimina ese gk de todos lados", 100);
        let d = d.unwrap();
        assert_eq!(d.provenance, "model-inferred");
        assert_eq!(c, Consulted::Note { cached: false });
        // Same prompt modulo case/whitespace: served from cache, no second call.
        let (d2, c2) = consult(&cfg(), &f, &s, "  elimina   ese GK de todos lados ", 101);
        assert_eq!(d2.unwrap(), d);
        assert_eq!(c2, Consulted::Note { cached: true });
        assert_eq!(*f.calls.borrow(), 1);
        // The cache stores a hash, never the prompt.
        let raw = std::fs::read_to_string(s.path.as_ref().unwrap()).unwrap();
        assert!(!raw.to_lowercase().contains("elimina"));
    }

    #[test]
    fn cache_is_bounded() {
        let s = store("bound");
        let mut st = State::default();
        for i in 0..200 {
            st.cache_put(format!("{i:016x}"), &task());
        }
        assert_eq!(st.cache.len(), MAX_CACHE);
        s.save(&st);
        assert_eq!(s.load().cache.len(), MAX_CACHE);
    }

    #[test]
    fn breaker_opens_after_two_failures_then_skips_then_recovers() {
        let s = store("breaker");
        let f = Fake::new(vec![
            Err(ClassifyError::Timeout),
            Err(ClassifyError::Failed("exit_status")),
        ]);
        let c = cfg();
        assert_eq!(
            consult(&c, &f, &s, "una frase dudosa uno", 1000).1,
            Consulted::Skipped("timeout")
        );
        assert!(!s.load().breaker_open(1000));
        assert_eq!(
            consult(&c, &f, &s, "una frase dudosa dos", 1001).1,
            Consulted::Skipped("exit_status")
        );
        assert!(s.load().breaker_open(1002));
        // Open: not even called.
        let before = *f.calls.borrow();
        assert_eq!(
            consult(&c, &f, &s, "una frase dudosa tres", 1100).1,
            Consulted::Skipped("breaker_open")
        );
        assert_eq!(*f.calls.borrow(), before);
        // After N minutes it tries again; a success closes it.
        f.answers.borrow_mut().push(Ok(task()));
        let (d, _) = consult(&c, &f, &s, "una frase dudosa cuatro", 1001 + 601);
        assert!(d.is_some());
        assert_eq!(s.load().fails, 0);
    }

    #[test]
    fn success_resets_the_failure_count_and_unavailable_does_not_trip() {
        let s = store("reset");
        let f = Fake::new(vec![
            Err(ClassifyError::Timeout),
            Ok(task()),
            Err(ClassifyError::Timeout),
            Err(ClassifyError::Unavailable("binary_not_found")),
            Err(ClassifyError::Unavailable("binary_not_found")),
        ]);
        let c = cfg();
        for (i, t) in [
            "a b c uno",
            "a b c dos",
            "a b c tres",
            "a b c cuatro",
            "a b c cinco",
        ]
        .iter()
        .enumerate()
        {
            let _ = consult(&c, &f, &s, t, 10 + i as u64);
        }
        assert!(!s.load().breaker_open(20));
    }

    #[test]
    fn disabled_config_never_calls() {
        let s = store("off");
        let f = Fake::new(vec![Ok(task())]);
        let mut c = cfg();
        c.enabled = false;
        assert_eq!(
            consult(&c, &f, &s, "hola mundo cruel", 1).1,
            Consulted::Skipped("disabled")
        );
        assert_eq!(*f.calls.borrow(), 0);
    }

    #[test]
    fn non_task_and_low_confidence_produce_no_note() {
        let s = store("nonote");
        let chat =
            parse_verdict(r#"{"intent":"chat","is_task":false,"confidence":0.95,"missing":[]}"#)
                .unwrap();
        let f = Fake::new(vec![Ok(chat)]);
        let (d, c) = consult(&cfg(), &f, &s, "ya quedó gracias por todo", 1);
        assert!(d.is_none());
        assert_eq!(c, Consulted::NoNote { cached: false });
    }

    #[test]
    fn prompt_sent_is_redacted_and_truncated() {
        struct Spy(RefCell<String>);
        impl IntentClassifier for Spy {
            fn classify(&self, p: &str) -> Result<ModelVerdict, ClassifyError> {
                *self.0.borrow_mut() = p.to_string();
                Err(ClassifyError::Timeout)
            }
        }
        let s = store("spy");
        let spy = Spy(RefCell::new(String::new()));
        let text = format!(
            "usa sk-ant-api03-{} {}",
            "B".repeat(40),
            "palabra ".repeat(500)
        );
        let _ = consult(&cfg(), &spy, &s, &text, 1);
        let sent = spy.0.borrow();
        assert!(!sent.contains("BBBBBBBB"), "secret leaked");
        assert!(sent.chars().count() <= 2000);
    }

    #[test]
    fn corrupt_state_is_ignored() {
        let s = store("corrupt");
        std::fs::write(s.path.as_ref().unwrap(), b"{not json").unwrap();
        assert_eq!(s.load(), State::default());
        let tampered = json!({"cache":[{"h":"x","v":"{\"intent\":\"hack\"}"}],"fails":"many"});
        let st = State::from_value(&tampered);
        assert!(st.cache_get("x").is_none());
        assert_eq!(st.fails, 0);
    }

    #[test]
    fn argv_has_no_tools_no_session_no_hooks_and_no_user_text() {
        let a = ClaudeCliClassifier::argv("haiku");
        let j = a.join(" ");
        assert!(
            j.contains("--tools  --effort low --no-session-persistence"),
            "{j}"
        );
        assert!(j.contains("disableAllHooks"));
        assert!(j.contains("--output-format json"));
        assert!(j.contains("--model haiku"));
    }

    #[test]
    fn config_model_alias_is_validated() {
        assert_eq!(sanitize_model("claude-haiku-4-5"), "claude-haiku-4-5");
        assert_eq!(sanitize_model(" opus "), "opus");
        for bad in ["", "a b", "x;rm", "--settings", "-p"] {
            assert_eq!(sanitize_model(bad), DEFAULT_MODEL, "{bad}");
        }
    }
}
