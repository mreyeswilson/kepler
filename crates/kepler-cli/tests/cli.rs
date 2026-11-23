#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

/// An isolated Kepler environment: own data dir, own IPC name, short idle timeout.
struct Env {
    home: PathBuf,
    ipc: String,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("kepler-cli-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            home,
            ipc: format!("cli{}-{n}", std::process::id()),
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
        c.args(args)
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_IPC_NAME", &self.ipc)
            .env("KEPLER_IDLE_TIMEOUT_SECS", "5")
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd(args).output().unwrap()
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let o = self.run(args);
        let stdout = String::from_utf8_lossy(&o.stdout);
        let v = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}"));
        (o.status.code().unwrap(), v)
    }

    fn daemon_running(&self) -> bool {
        let (_, v) = self.json(&["doctor", "--json"]);
        v["ipc"]["daemon"]["running"] == true
    }

    fn wait_for_daemon_exit(&self) {
        // Probing connects to the daemon and therefore resets its idle timer, so sleep
        // longer than the idle timeout between probes instead of polling quickly.
        let start = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(6));
            if !self.daemon_running() {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "daemon did not idle-shutdown"
            );
        }
    }
}

const INPUT: &str = r#"{"version":1,"id":"e1","type":"input.received","timestamp":"2026-01-02T03:04:05Z","harness":"pi","project_id":"p","session_id":"s1","correlation_id":"c","payload":{"text":"Fix the failing test in parser.rs"}}"#;
const READ_ENV: &str = r#"{"version":1,"id":"e2","type":"tool.call.requested","timestamp":"2026-01-02T03:04:05Z","harness":"pi","project_id":"p","session_id":"s1","correlation_id":"c","payload":{"tool":"bash","command":"cat .env"}}"#;

#[test]
fn version_command() {
    let env = Env::new();
    let o = env.run(&["version"]);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(s.starts_with("kepler "), "{s}");
    assert!(s.contains("ipc protocol 1") && s.contains("event schema 1"));
    let (_, v) = env.json(&["version", "--json"]);
    assert_eq!(v["protocol"], 1);
    assert_eq!(v["event_schema"], 1);
}

#[test]
fn doctor_reports_without_creating_anything() {
    let env = Env::new();
    let (code, v) = env.json(&["doctor", "--json"]);
    assert_eq!(code, 0);
    assert_eq!(v["ipc"]["daemon"]["running"], false);
    assert_eq!(v["database"]["exists"], false);
    assert_eq!(v["capabilities"]["contract"].as_array().unwrap().len(), 8);
    assert_eq!(v["runtime"]["llm_or_network_calls"], false);
    assert!(
        !env.home.join("kepler.db").exists(),
        "doctor must not create the database"
    );
    let text = String::from_utf8_lossy(&env.run(&["doctor"]).stdout).to_string();
    for needle in [
        "Runtime",
        "IPC",
        "Database",
        "Capabilities",
        "Adapters",
        "not running",
    ] {
        assert!(text.contains(needle), "missing {needle} in:\n{text}");
    }
}

#[test]
fn emit_in_process_needs_no_daemon() {
    let env = Env::new();
    let (code, v) = env.json(&["emit", "--in-process", "--json", INPUT]);
    assert_eq!(code, 0);
    assert_eq!(v["action"], "passthrough");
    let (_, v) = env.json(&["emit", "--in-process", "--json", READ_ENV]);
    assert_eq!(v["action"], "deny");
    assert!(!env.daemon_running());
}

#[test]
fn missing_core_is_passthrough_with_exit_zero() {
    let env = Env::new();
    let (code, v) = env.json(&["emit", "--no-autostart", "--json", INPUT]);
    assert_eq!(code, 0, "missing core must never fail the caller");
    assert_eq!(v["action"], "passthrough");
    assert_eq!(v["reason"], "core_unavailable");
}

#[test]
fn invalid_events_exit_two_without_starting_a_daemon() {
    let env = Env::new();
    for bad in ["not json", r#"{"version":2}"#, "{}"] {
        let (code, v) = env.json(&["emit", "--json", bad]);
        assert_eq!(code, 2, "{bad}");
        assert_eq!(v["action"], "rejected");
    }
    assert!(!env.daemon_running());
}

#[test]
fn stdin_and_file_sources_work() {
    let env = Env::new();
    let mut child = env
        .cmd(&["emit", "--in-process"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(INPUT.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("passthrough"));

    let f = env.home.join("event.json");
    std::fs::write(&f, READ_ENV).unwrap();
    let (_, v) = env.json(&["emit", "--in-process", "--file", f.to_str().unwrap()]);
    assert_eq!(v["action"], "deny");
}

#[test]
fn oversized_event_is_refused_by_the_cli() {
    let env = Env::new();
    let big = format!("{{\"pad\":\"{}\"}}", "x".repeat(70_000));
    // Passed through stdin: a 70 KB argument would exceed Windows command-line limits.
    let mut child = env
        .cmd(&["emit", "--in-process"])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The CLI stops reading at the limit and exits; a broken pipe on our side is expected.
    let _ = child.stdin.take().unwrap().write_all(big.as_bytes());
    assert_eq!(child.wait().unwrap().code(), Some(2));
}

#[test]
fn end_to_end_lazy_daemon_passthrough_persistence_and_idle_shutdown() {
    let env = Env::new();
    assert!(!env.daemon_running());

    // First event lazily starts the daemon and still returns a decision.
    let (code, v) = env.json(&["emit", "--json", INPUT]);
    assert_eq!(code, 0);
    assert_eq!(v["action"], "passthrough", "{v}");
    assert_eq!(
        v["reason"], "no_enrichment",
        "real daemon decision expected, got {v}"
    );
    assert!(env.daemon_running(), "daemon should now be running");

    // Same daemon answers the security preflight (fail closed on .env).
    let (_, v) = env.json(&["emit", "--no-autostart", "--json", READ_ENV]);
    assert_eq!(v["action"], "deny", "{v}");

    // doctor sees the running daemon and the freshly created database.
    let (_, d) = env.json(&["doctor", "--json"]);
    assert_eq!(d["ipc"]["daemon"]["running"], true);
    assert_eq!(d["database"]["exists"], true);
    assert_eq!(d["database"]["journal_mode"], "wal");
    assert_eq!(d["database"]["fts5"], true);

    // No clients + 5s idle timeout => the daemon exits on its own.
    env.wait_for_daemon_exit();
    let (code, v) = env.json(&["emit", "--no-autostart", "--json", INPUT]);
    assert_eq!(code, 0);
    assert_eq!(v["reason"], "core_unavailable");
    // clean up: next test run must not see stale daemons
    assert!(!env.daemon_running());
}
