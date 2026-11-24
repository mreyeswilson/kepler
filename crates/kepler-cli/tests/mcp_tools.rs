//! End-to-end tests of the MCP tools through the real `kepler mcp serve` binary over stdio
//! (initialize -> tools/list -> tools/call). Orbit runs use real git repositories and a fake
//! sub-agent (cargo example `fake_agent`, chosen with `KEPLER_AGENT_BIN`); nothing here talks to
//! a model or the network.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

fn fake_agent() -> PathBuf {
    // Not derived from current_exe(): see forge_model.rs.
    let mut p = PathBuf::from(env!("CARGO_BIN_EXE_kepler"));
    p.pop();
    p.push("examples");
    p.push(format!("fake_agent{}", std::env::consts::EXE_SUFFIX));
    assert!(p.exists(), "build the example first: {}", p.display());
    p
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&o.stdout).into_owned()
}

struct Env {
    home: PathBuf,
    ipc: String,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("kepler-mcp-e2e-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            home,
            ipc: format!("mcp{}-{n}", std::process::id()),
        }
    }

    fn repo(&self) -> PathBuf {
        let repo = self.home.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("base.txt"), "base\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    fn serve(&self, extra_env: &[(&str, &str)]) -> Client {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kepler"));
        cmd.args(["mcp", "serve"])
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_IPC_NAME", &self.ipc)
            .env("KEPLER_AGENT_BIN", fake_agent())
            .env("FAKE_AGENT_LOG", self.home.join("agent.log"))
            .env_remove("KEPLER_MCP_READONLY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut c = Client {
            child,
            stdin,
            rx,
            next: 1,
        };
        let r = c.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}}),
        );
        assert_eq!(r["result"]["serverInfo"]["name"], "kepler");
        c.notify("notifications/initialized");
        c
    }
}

struct Client {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next: u64,
}

impl Client {
    fn send(&mut self, v: &Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let line = self
            .rx
            .recv_timeout(Duration::from_secs(60))
            .unwrap_or_else(|_| panic!("no reply to {method}"));
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id, "{line}");
        v
    }

    /// `tools/call`; returns (isError, parsed-or-raw text). Protocol errors panic.
    fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        assert!(r.get("error").is_none(), "{name}: protocol error {r}");
        (
            r["result"]["isError"].as_bool().unwrap(),
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }

    fn json_tool(&mut self, name: &str, args: Value) -> Value {
        let (err, text) = self.tool(name, args);
        assert!(!err, "{name} failed: {text}");
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name}: {e}: {text}"))
    }

    fn wait_status(&mut self, run: &str, want: &[&str]) -> Value {
        let start = Instant::now();
        loop {
            let v = self.json_tool("kepler_orbit_status", json!({"run": run}));
            if want.contains(&v["status"].as_str().unwrap()) {
                return v;
            }
            assert!(
                start.elapsed() < Duration::from_secs(90),
                "run never reached {want:?}: {v}"
            );
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn task<'a>(status: &'a Value, id: &str) -> &'a Value {
    status["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}

#[test]
fn tools_list_has_every_tool_with_annotations_over_real_stdio() {
    let env = Env::new();
    let mut c = env.serve(&[]);
    let r = c.request("tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    assert_eq!(
        tools.len(),
        24,
        "{}",
        tools
            .iter()
            .map(|t| t["name"].to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    for t in tools {
        assert!(t["annotations"]["readOnlyHint"].is_boolean(), "{t}");
        assert_eq!(t["inputSchema"]["additionalProperties"], false, "{t}");
    }
    // Unknown fields and wrong types are protocol errors, not tool errors.
    let r = c.request(
        "tools/call",
        json!({"name": "kepler_forge_analyze", "arguments": {"text": 5}}),
    );
    assert_eq!(r["error"]["code"], -32602);
    let r = c.request(
        "tools/call",
        json!({"name": "kepler_orbit_status", "arguments": {"run": "orb-1", "x": 1}}),
    );
    assert_eq!(r["error"]["code"], -32602);
}

#[test]
fn forge_enriches_a_vague_prompt_and_guardian_decides() {
    let env = Env::new();
    let mut c = env.serve(&[]);
    let f = c.json_tool(
        "kepler_forge_analyze",
        json!({"text": "arregla el bug del login"}),
    );
    assert_eq!(f["mode"], "enrich", "{f}");
    assert!(f["disclaimer"]
        .as_str()
        .unwrap()
        .contains("NOT user instructions"));
    let g = c.json_tool("kepler_guardian_check", json!({"command": "rm -rf /"}));
    assert_eq!(g["verdict"], "ask", "{g}");
    let g = c.json_tool("kepler_guardian_check", json!({"path": ".env"}));
    assert_eq!(g["verdict"], "deny", "{g}");
    let g = c.json_tool("kepler_guardian_check", json!({"command": "cargo test"}));
    assert_eq!(g["verdict"], "allow", "{g}");
    let (err, text) = c.tool("kepler_guardian_check", json!({}));
    assert!(err && text.contains("exactly one"), "{text}");
}

#[test]
fn orbit_plan_validates_without_running_and_reports_invalid_plans() {
    let env = Env::new();
    let mut c = env.serve(&[]);
    let ok = c.json_tool(
        "kepler_orbit_plan",
        json!({"plan": {"name": "dag", "tasks": [
            {"id": "a", "command": ["git", "--version"]},
            {"id": "b", "command": ["git", "--version"], "depends_on": ["a"]}]}}),
    );
    assert_eq!(ok["order"], json!(["a", "b"]));
    assert_eq!(ok["waves"], json!([["a"], ["b"]]));
    let (err, text) = c.tool(
        "kepler_orbit_plan",
        json!({"plan": {"name": "loop", "tasks": [
            {"id": "a", "command": ["git", "--version"], "depends_on": ["b"]},
            {"id": "b", "command": ["git", "--version"], "depends_on": ["a"]}]}}),
    );
    assert!(err && text.contains("cycle"), "{text}");
    let (err, text) = c.tool(
        "kepler_orbit_plan",
        json!({"plan": {"name": "env", "tasks": [{"id": "a", "command": ["cat", ".env"]}]}}),
    );
    assert!(err && text.contains("Guardian"), "{text}");
    // Nothing was created.
    let list = c.json_tool("kepler_orbit_list", json!({}));
    assert_eq!(list["runs"].as_array().unwrap().len(), 0);
}

#[test]
fn orbit_run_returns_at_once_then_approval_needs_confirm_and_cleanup_removes_branches() {
    let env = Env::new();
    let repo = env.repo();
    let mut c = env.serve(&[]);
    let plan = json!({"name": "demo", "tasks": [
        {"id": "first", "command": ["git", "--version"]},
        {"id": "second", "command": ["git", "log", "--oneline"], "depends_on": ["first"]},
        {"id": "wipe", "command": ["git", "clean", "-fd"], "depends_on": ["first"]}]});
    let started = Instant::now();
    let run = c.json_tool(
        "kepler_orbit_run",
        json!({"plan": plan, "repo": repo.to_str().unwrap()}),
    );
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "run must not block"
    );
    let id = run["run"].as_str().unwrap().to_string();
    assert!(id.starts_with("orb-"), "{run}");

    let st = c.wait_status(&id, &["needs_approval", "done", "failed"]);
    assert_eq!(st["status"], "needs_approval", "{st}");
    assert_eq!(task(&st, "second")["status"], "done");
    assert_eq!(task(&st, "wipe")["status"], "awaiting_approval");
    assert!(st["hint"].as_str().unwrap().contains("approval"));

    // The schema demands `confirm`; `confirm: false` is refused with an explanation.
    let r = c.request(
        "tools/call",
        json!({"name": "kepler_orbit_approve", "arguments": {"run": id, "task": "wipe"}}),
    );
    assert_eq!(r["error"]["code"], -32602);
    let (err, text) = c.tool(
        "kepler_orbit_approve",
        json!({"run": id, "task": "wipe", "confirm": false}),
    );
    assert!(err && text.contains("explicitly approved"), "{text}");
    assert_eq!(
        task(
            &c.json_tool("kepler_orbit_status", json!({"run": id})),
            "wipe"
        )["status"],
        "awaiting_approval"
    );

    // Ids with path tricks never reach the filesystem.
    let (err, _) = c.tool("kepler_orbit_status", json!({"run": "..\\..\\x"}));
    assert!(err);
    let (err, _) = c.tool("kepler_orbit_logs", json!({"run": id, "task": "../x"}));
    assert!(err);

    let ap = c.json_tool(
        "kepler_orbit_approve",
        json!({"run": id, "task": "wipe", "confirm": true}),
    );
    assert_eq!(ap["decision"], "approved");
    let (err, _) = c.tool(
        "kepler_orbit_approve",
        json!({"run": id, "task": "wipe", "confirm": true}),
    );
    assert!(err, "approval is one-shot");
    c.json_tool("kepler_orbit_resume", json!({"run": id}));
    let st = c.wait_status(&id, &["done", "failed", "needs_approval"]);
    assert_eq!(st["status"], "done", "{st}");
    assert_eq!(task(&st, "wipe")["status"], "done");
    let logs = c.tool("kepler_orbit_logs", json!({"run": id, "task": "first"}));
    assert!(!logs.0 && logs.1.contains("git version"), "{}", logs.1);

    let listed = c.json_tool("kepler_orbit_list", json!({}));
    assert!(listed["runs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["run"] == id.as_str()));

    let (err, text) = c.tool("kepler_orbit_cleanup", json!({"run": id, "confirm": false}));
    assert!(err && text.contains("nothing removed"), "{text}");
    let cl = c.json_tool("kepler_orbit_cleanup", json!({"run": id, "confirm": true}));
    assert!(
        cl["removed_worktrees_and_branches"].as_u64().unwrap() >= 1,
        "{cl}"
    );
    assert!(git(&repo, &["branch", "--list", "kepler/orbit/*"])
        .trim()
        .is_empty());
}

#[test]
fn deny_skips_dependents_and_cancel_is_refused_when_nothing_runs() {
    let env = Env::new();
    let repo = env.repo();
    let mut c = env.serve(&[]);
    let plan = json!({"name": "deny", "tasks": [
        {"id": "wipe", "command": ["git", "clean", "-fd"]},
        {"id": "after", "command": ["git", "--version"], "depends_on": ["wipe"]}]});
    let run = c.json_tool(
        "kepler_orbit_run",
        json!({"plan": plan, "repo": repo.to_str().unwrap()}),
    );
    let id = run["run"].as_str().unwrap().to_string();
    c.wait_status(&id, &["needs_approval"]);
    let d = c.json_tool("kepler_orbit_deny", json!({"run": id, "task": "wipe"}));
    assert_eq!(d["decision"], "denied");
    c.json_tool("kepler_orbit_resume", json!({"run": id}));
    let st = c.wait_status(&id, &["failed", "done"]);
    assert_eq!(task(&st, "wipe")["status"], "denied", "{st}");
    assert_eq!(task(&st, "after")["status"], "skipped", "{st}");
    let (err, text) = c.tool("kepler_orbit_cancel", json!({"run": id}));
    assert!(err && text.contains("not running"), "{text}");
}

#[test]
fn divide_and_delegate_two_agents_then_an_integration_task_through_a_fake_agent() {
    let env = Env::new();
    let repo = env.repo();
    let mut c = env.serve(&[]);
    // The model decides the split; Kepler builds the plan deterministically.
    let s = c.json_tool(
        "kepler_orbit_suggest_plan",
        json!({"objective": "Add two files then integrate", "subtasks": [
            {"id": "alpha", "title": "alpha file", "prompt": "write alpha.txt hello from alpha"},
            {"id": "beta", "title": "beta file", "prompt": "write beta.txt hello from beta"}],
            "integration_prompt": "integrate", "max_parallel": 2}),
    );
    assert_eq!(s["validation"]["ok"], true, "{s}");
    assert_eq!(
        s["validation"]["waves"],
        json!([["alpha", "beta"], ["integrate"]])
    );
    assert_eq!(s["validation"]["agent_tasks"], 3);
    let run = c.json_tool(
        "kepler_orbit_run",
        json!({"plan": s["plan"], "repo": repo.to_str().unwrap()}),
    );
    let id = run["run"].as_str().unwrap().to_string();
    let st = c.wait_status(&id, &["done", "failed", "needs_approval"]);
    assert_eq!(st["status"], "done", "{st}");
    for t in ["alpha", "beta", "integrate"] {
        assert_eq!(task(&st, t)["status"], "done", "{t}: {st}");
    }
    assert!(st["overlaps"].as_array().unwrap().is_empty());
    // Real commits on real branches; the user's working tree was not touched.
    let alpha = git(
        &repo,
        &["show", &format!("kepler/orbit/{id}/alpha:alpha.txt")],
    );
    assert_eq!(alpha.trim(), "hello from alpha");
    let integrated = git(
        &repo,
        &[
            "show",
            &format!("kepler/orbit/{id}/integrate:integrated.txt"),
        ],
    );
    assert!(
        integrated.contains("alpha.txt") && integrated.contains("beta.txt"),
        "{integrated}"
    );
    assert!(!repo.join("alpha.txt").exists());
    assert!(git(&repo, &["status", "--porcelain"]).trim().is_empty());
    // The agent got `-p <prompt>` plus the default permission mode, as argv.
    let log: String = std::fs::read_dir(&env.home)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("agent.log."))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert!(
        log.contains(
            r#"["-p","write alpha.txt hello from alpha","--permission-mode","acceptEdits"]"#
        ),
        "{log}"
    );
    c.json_tool(
        "kepler_orbit_cleanup",
        json!({"run": id, "confirm": true, "delete_records": true}),
    );
}

#[test]
fn agent_logs_are_redacted_and_tail_limited_and_a_failing_agent_is_reported() {
    let env = Env::new();
    let repo = env.repo();
    let mut c = env.serve(&[]);
    let plan = json!({"name": "chatty", "tasks": [
        {"id": "talk", "agent": "chatty"},
        {"id": "boom", "agent": "fail"}]});
    let run = c.json_tool(
        "kepler_orbit_run",
        json!({"plan": plan, "repo": repo.to_str().unwrap()}),
    );
    let id = run["run"].as_str().unwrap().to_string();
    let st = c.wait_status(&id, &["done", "failed", "needs_approval"]);
    assert_eq!(st["status"], "failed", "{st}");
    assert_eq!(task(&st, "talk")["status"], "done");
    assert_eq!(task(&st, "boom")["status"], "failed");
    assert_eq!(task(&st, "boom")["exit_code"], 7);
    let (err, text) = c.tool(
        "kepler_orbit_logs",
        json!({"run": id, "task": "talk", "tail_bytes": 4000}),
    );
    assert!(!err);
    assert!(!text.contains("ghp_a1B2"), "secret leaked");
    assert!(
        text.contains("earlier bytes omitted") && text.contains("line 01999"),
        "{}",
        &text[..200.min(text.len())]
    );
    assert!(text.len() < 5000);
    let (_, text) = c.tool(
        "kepler_orbit_logs",
        json!({"run": id, "task": "boom", "stream": "stderr"}),
    );
    assert!(text.contains("failed on purpose"));
}

#[test]
fn memory_sentinel_vision_tools_and_the_response_size_limit() {
    let env = Env::new();
    let repo = env.repo();
    let mut c = env.serve(&[]);

    // memory: add (agent-derived), list, export
    let added = c.json_tool(
        "kepler_memory_add",
        json!({"text": "the release branch is stable-2", "project": "p1"}),
    );
    assert_eq!(added["source"], "derived");
    let listed = c.json_tool("kepler_memory_list", json!({"project": "p1"}));
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["source"], "derived", "{listed}");
    let (err, _) = c.tool(
        "kepler_memory_add",
        json!({"text": "x", "project": "p1", "scope": "branch"}),
    );
    assert!(err, "branch scope needs a key");
    for i in 0..40 {
        // Distinct words per note, so consolidation does not merge them.
        let words: Vec<String> = (0..120).map(|j| format!("w{i}x{j}")).collect();
        let text = format!("note {i}: {}", words.join(" "));
        c.json_tool("kepler_memory_add", json!({"text": text, "project": "p1"}));
    }
    let (err, export) = c.tool("kepler_memory_export", json!({"project": "p1"}));
    assert!(!err);
    assert!(export.len() <= 32 * 1024 + 64, "{}", export.len());
    assert!(
        export.ends_with("[output truncated by kepler]"),
        "export was not truncated"
    );
    let found = c.tool(
        "kepler_memory_search",
        json!({"query": "release branch", "project": "p1"}),
    );
    assert!(found.1.contains("stable-2"), "{}", found.1);

    // sentinel: audit sees a secret-shaped line added to a tracked file; check stores findings
    let key = format!("ghp_{}", "a1B2".repeat(9));
    std::fs::write(repo.join("base.txt"), format!("base\ntoken = \"{key}\"\n")).unwrap();
    let a = c.json_tool(
        "kepler_sentinel_audit",
        json!({"repo": repo.to_str().unwrap()}),
    );
    assert!(a["finding_count"].as_u64().unwrap() >= 1, "{a}");
    assert!(!a.to_string().contains(&key), "secret must be redacted");
    let (err, _) = c.tool(
        "kepler_sentinel_audit",
        json!({"repo": repo.to_str().unwrap(), "staged": true, "base": "HEAD"}),
    );
    assert!(err, "staged and base are exclusive");
    let (err, _) = c.tool(
        "kepler_sentinel_audit",
        json!({"repo": "C:\\definitely\\not\\here"}),
    );
    assert!(err);
    let chk = c.json_tool(
        "kepler_sentinel_check",
        json!({"repo": repo.to_str().unwrap(), "name": "gitv", "argv": ["git", "--version"]}),
    );
    assert_eq!(chk["status"], "passed", "{chk}");
    let (err, _) = c.tool(
        "kepler_sentinel_check",
        json!({"repo": repo.to_str().unwrap(), "name": "bad", "argv": ["curl", "-H", key]}),
    );
    assert!(err, "secret-shaped argv is refused");
    let l = c.json_tool("kepler_sentinel_list", json!({}));
    assert!(l.is_array());

    // vision: status never starts a worker; non-local hosts are refused
    let v = c.json_tool("kepler_vision_status", json!({}));
    assert_eq!(v["started"], false);
    let (err, text) = c.tool("kepler_vision_check", json!({"url": "http://example.com/"}));
    assert!(err, "{text}");
    let (err, _) = c.tool(
        "kepler_vision_check",
        json!({"url": "file:///c:/windows/win.ini"}),
    );
    assert!(err);
}

#[test]
fn read_only_mode_and_oversized_input_fail_closed() {
    let env = Env::new();
    let repo = env.repo();
    let mut c = env.serve(&[("KEPLER_MCP_READONLY", "1")]);
    let r = c.request("tools/list", json!({}));
    let names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"kepler_orbit_plan") && !names.contains(&"kepler_orbit_run"),
        "{names:?}"
    );
    let (err, text) = c.tool(
        "kepler_orbit_run",
        json!({"plan": {"name": "p", "tasks": [{"id": "a", "command": ["git", "--version"]}]}, "repo": repo.to_str().unwrap()}),
    );
    assert!(err && text.contains("KEPLER_MCP_READONLY"), "{text}");
    assert_eq!(
        c.json_tool("kepler_orbit_list", json!({}))["runs"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // An input line over the limit is rejected without being parsed, and the server survives.
    let big = format!(
        r#"{{"jsonrpc":"2.0","id":99,"method":"ping","pad":"{}"}}"#,
        "a".repeat(1024 * 1024 + 10)
    );
    writeln!(c.stdin, "{big}").unwrap();
    let line = c.rx.recv_timeout(Duration::from_secs(30)).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["error"]["code"], -32600, "{line}");
    assert_eq!(c.request("ping", json!({}))["result"], json!({}));
    // Text over the schema limit is a protocol error (16 KiB for Forge).
    let r = c.request(
        "tools/call",
        json!({"name": "kepler_forge_analyze", "arguments": {"text": "x".repeat(20_000)}}),
    );
    assert_eq!(r["error"]["code"], -32602);
}
