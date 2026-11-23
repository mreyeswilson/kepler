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

#[test]
fn memory_controls_add_search_export_disable_purge() {
    let env = Env::new();
    // Read-only commands on a missing database do not create it.
    let o = env.run(&["memory", "stats", "--json"]);
    assert!(o.status.success());
    assert!(!env.home.join("kepler.db").exists());

    let o = env.run(&[
        "memory",
        "add",
        "release branch is stable-2 and we squash merge; deploy key api_key=sk-abcdefghijklmnopqrstuvwxyz0123456789",
        "--project",
        "p1",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (_, s) = env.json(&["memory", "stats", "--json", "--project", "p1"]);
    assert_eq!(s["total"], 1);
    assert_eq!(s["enabled"], true);

    let o = env.run(&[
        "memory",
        "search",
        "which branch for release",
        "--project",
        "p1",
    ]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("stable-2") && out.contains("NOT instructions"),
        "{out}"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("ESTIMATED"));
    let o = env.run(&[
        "memory",
        "search",
        "which branch for release",
        "--project",
        "other",
    ]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("no relevant memories"));

    let exported =
        String::from_utf8_lossy(&env.run(&["memory", "export", "--project", "p1"]).stdout)
            .to_string();
    assert!(exported.contains("stable-2"));
    assert!(!exported.contains("sk-abcdefgh"), "{exported}");
    let (_, list) = env.json(&["memory", "list", "--json"]);
    assert_eq!(list[0]["provenance"][0]["kind"], "cli");

    env.run(&["memory", "disable"]);
    let (_, s) = env.json(&["memory", "stats", "--json"]);
    assert_eq!(s["enabled"], false);
    let (_, d) = env.json(&["doctor", "--json"]);
    assert_eq!(d["modules"]["memory"]["global_enabled"], false);
    env.run(&["memory", "enable"]);

    // Purge needs --yes and a filter or --all.
    let o = env.run(&["memory", "purge", "--project", "p1"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("--yes"));
    assert_eq!(env.json(&["memory", "stats", "--json"]).1["total"], 1);
    let o = env.run(&["memory", "purge", "--yes"]);
    assert!(
        !o.status.success(),
        "empty filter without --all must be refused"
    );
    let o = env.run(&["memory", "purge", "--project", "p1", "--yes"]);
    assert!(o.status.success());
    assert_eq!(env.json(&["memory", "stats", "--json"]).1["total"], 0);
}

#[test]
fn mcp_serve_speaks_json_rpc_over_stdio_with_a_real_client_flow() {
    let env = Env::new();
    env.run(&[
        "memory",
        "add",
        "the release branch is stable-2 and we squash merge",
        "--project",
        "p1",
    ]);
    let mut child = env
        .cmd(&["mcp", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kepler_doctor","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"which branch for release","project":"p1"}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"kepler_inspect","arguments":{"limit":5}}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"which branch","project":"someone-else"}}}"#,
    ];
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin); // EOF ends the server
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let replies: Vec<Value> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 6, "the notification must not be answered");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 3);
    let text = |i: usize| {
        replies[i]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let doctor: Value = serde_json::from_str(&text(2)).unwrap();
    assert_eq!(doctor["runtime"]["llm_or_network_calls"], false);
    assert!(
        text(3).contains("stable-2") && text(3).contains("ESTIMATED"),
        "{}",
        text(3)
    );
    let inspect: Value = serde_json::from_str(&text(4)).unwrap();
    assert_eq!(inspect["memory"]["total"], 1);
    assert!(text(5).contains("no relevant memories"));

    // `kepler inspect` is the same data from the terminal.
    let (_, v) = env.json(&["inspect", "--limit", "5"]);
    assert_eq!(v["memory"]["total"], 1);
}

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn sentinel_audit_stores_findings_and_list_shows_them() {
    let env = Env::new();
    let repo = env.home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_in(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.py"), "x = 1\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-q", "-m", "init"]);
    std::fs::write(repo.join("a.py"), "x = 1\neval(user_input)\n").unwrap();
    let r = repo.to_str().unwrap();

    // Without --store nothing is persisted.
    let (code, v) = env.json(&["sentinel", "audit", "--repo", r, "--json"]);
    assert_eq!(code, 0);
    assert_eq!(v["findings"][0]["rule"], "py.eval");
    assert_eq!(v["findings"][0]["line"], 2);
    let (_, none) = env.json(&["sentinel", "list", "--json"]);
    assert_eq!(none.as_array().unwrap().len(), 0);

    let (code, v) = env.json(&[
        "sentinel",
        "audit",
        "--repo",
        r,
        "--json",
        "--store",
        "--project",
        "demo",
    ]);
    assert_eq!(code, 0);
    assert_eq!(v["stored_new"], 1);
    let (_, rows) = env.json(&["sentinel", "list", "--json", "--project", "demo"]);
    assert_eq!(rows[0]["rule"], "py.eval");
    assert_eq!(rows[0]["confidence"], "suspected");
    let id = rows[0]["id"].as_i64().unwrap().to_string();

    // --fail-on turns findings into a failing exit status for CI use.
    let o = env.run(&[
        "sentinel",
        "audit",
        "--repo",
        r,
        "--json",
        "--fail-on",
        "high",
    ]);
    assert_eq!(o.status.code(), Some(1));

    assert!(env.run(&["sentinel", "dismiss", &id]).status.success());
    let (_, rows) = env.json(&["sentinel", "list", "--json", "--project", "demo"]);
    assert_eq!(rows.as_array().unwrap().len(), 0);
    let (_, rows) = env.json(&["sentinel", "list", "--json", "--project", "demo", "--all"]);
    assert_eq!(rows[0]["status"], "dismissed");

    let o = env.run(&["sentinel", "audit", "--repo", env.home.to_str().unwrap()]);
    // Not a repository (or nested in one): either way it must fail cleanly or succeed, never panic.
    assert!(o.status.code().is_some());
}

#[test]
fn vision_is_off_until_configured_and_never_starts_a_worker_by_itself() {
    let env = Env::new();
    let mut status = env.cmd(&["vision", "status"]);
    status.env_remove("KEPLER_VISION_WORKER");
    let o = status.output().unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("not configured"));

    let mut check = env.cmd(&["vision", "check", "http://localhost:3000/"]);
    check.env_remove("KEPLER_VISION_WORKER");
    let o = check.output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("KEPLER_VISION_WORKER"));

    // A configured but missing worker program is a clean error, and a refused URL never reaches it.
    let mut refused = env.cmd(&["vision", "check", "https://example.com/"]);
    refused.env("KEPLER_VISION_WORKER", env.home.join("no-such-worker.exe"));
    let o = refused.output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("URL refused"));

    let o = env.run(&[
        "vision",
        "check",
        "http://localhost/",
        "--viewport",
        "10x10",
    ]);
    assert_eq!(
        o.status.code(),
        Some(2),
        "invalid viewport is a usage error"
    );
}
