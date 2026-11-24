//! End-to-end tests for the in-flow features of `kepler hook claude-code` (ADR-0027):
//! Sentinel on Stop, Memory across sessions, Guardian token optimization. Real daemon, real git.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static N: AtomicU64 = AtomicU64::new(0);

struct Env {
    home: PathBuf,
    ipc: String,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("kepler-flow-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        Self {
            home,
            ipc: format!("fl{}-{n}", std::process::id()),
            root,
        }
    }

    fn project(&self, name: &str) -> PathBuf {
        let d = self.root.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
        c.args(args)
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_IPC_NAME", &self.ipc)
            .env("KEPLER_IDLE_TIMEOUT_SECS", "4")
            .env("KEPLER_FORGE_MODEL", "off")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("KEPLER_GUARDIAN_RULES")
            .env_remove("KEPLER_GUARDIAN_TOKENS")
            .env_remove("KEPLER_SENTINEL_CHECK")
            .env_remove("KEPLER_MEMORY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn run(&self, mut c: Command, stdin: &str) -> Output {
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn hook_env(&self, event: &str, input: &Value, envs: &[(&str, &str)]) -> Option<Value> {
        let mut c = self.cmd(&["hook", "claude-code", event]);
        for (k, v) in envs {
            c.env(k, v);
        }
        let o = self.run(c, &input.to_string());
        assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
        let out = String::from_utf8_lossy(&o.stdout);
        (!out.trim().is_empty())
            .then(|| serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}")))
    }

    fn hook(&self, event: &str, input: &Value) -> Option<Value> {
        self.hook_env(event, input, &[])
    }

    fn cli(&self, args: &[&str]) -> String {
        let o = self.run(self.cmd(args), "");
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn pause() {
    std::thread::sleep(Duration::from_millis(400));
}

fn ctx(v: &Option<Value>) -> String {
    v.as_ref()
        .and_then(|v| v["hookSpecificOutput"]["additionalContext"].as_str())
        .unwrap_or("")
        .to_string()
}

fn git(dir: &Path, args: &[&str]) {
    let s = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(s.status.success(), "{args:?}: {}", String::from_utf8_lossy(&s.stderr));
}

fn payload(dir: &Path, session: &str, event: &str) -> Value {
    json!({"session_id": session, "cwd": dir.to_string_lossy(), "hook_event_name": event})
}

fn with(mut v: Value, extra: Value) -> Value {
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[test]
fn memory_survives_sessions_and_stays_inside_the_project() {
    let e = Env::new();
    let proj = e.project("alpha");
    let other = e.project("beta");
    git(&proj, &["init", "-q"]);
    std::fs::write(proj.join("Cargo.toml"), "[package]\n").unwrap();

    // Session 1: the user states a durable fact; the turn settles.
    e.hook("SessionStart", &payload(&proj, "s1", "SessionStart"));
    e.hook(
        "UserPromptSubmit",
        &with(
            payload(&proj, "s1", "UserPromptSubmit"),
            json!({"prompt": "Recuerda que usamos pnpm y no npm en este proyecto"}),
        ),
    );
    e.hook("Stop", &payload(&proj, "s1", "Stop"));
    e.hook("SessionEnd", &payload(&proj, "s1", "SessionEnd"));
    pause();

    // Session 2, same project: the brief at SessionStart carries it, labeled.
    let start = e.hook("SessionStart", &payload(&proj, "s2", "SessionStart"));
    let c = ctx(&start);
    assert!(c.contains("usamos pnpm y no npm"), "{c}");
    assert!(c.contains("user-stated"), "{c}");
    assert!(c.contains("project memory"), "{c}");
    // Observed facts (stack) were consolidated at Stop with their own label.
    assert!(c.contains("observed") && c.contains("Rust"), "{c}");
    // ...and relevant retrieval at the prompt.
    let p = e.hook(
        "UserPromptSubmit",
        &with(
            payload(&proj, "s2", "UserPromptSubmit"),
            json!({"prompt": "que gestor de paquetes usamos aqui, pnpm o npm?"}),
        ),
    );
    assert!(ctx(&p).contains("usamos pnpm y no npm"), "{}", ctx(&p));

    // Another project: nothing.
    let o = e.hook("SessionStart", &payload(&other, "s3", "SessionStart"));
    assert!(!ctx(&o).contains("pnpm"), "{}", ctx(&o));
    let o = e.hook(
        "UserPromptSubmit",
        &with(
            payload(&other, "s3", "UserPromptSubmit"),
            json!({"prompt": "que gestor de paquetes usamos aqui, pnpm o npm?"}),
        ),
    );
    assert!(!ctx(&o).contains("pnpm"), "{}", ctx(&o));

    // KEPLER_MEMORY=off in the hook's environment: no brief.
    let off = e.hook_env(
        "SessionStart",
        &payload(&proj, "s4", "SessionStart"),
        &[("KEPLER_MEMORY", "off")],
    );
    assert!(!ctx(&off).contains("pnpm"));

    // Controls cover the new memories.
    let list = e.cli(&["memory", "list"]);
    assert!(list.contains("usamos pnpm"), "{list}");
}

#[test]
fn sentinel_audits_on_stop_and_hands_findings_to_the_next_prompt_once() {
    let e = Env::new();
    let proj = e.project("repo");
    git(&proj, &["init", "-q"]);
    std::fs::write(proj.join("a.rs"), "fn main() {}\n").unwrap();
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-q", "-m", "init"]);

    e.hook("SessionStart", &payload(&proj, "s1", "SessionStart"));
    // The agent edits a file and introduces a token-looking secret.
    let token = format!("ghp_{}", "a1B2".repeat(9));
    std::fs::write(proj.join("a.rs"), format!("fn main() {{}}\nconst T: &str = \"{token}\";\n")).unwrap();
    let post = e.hook(
        "PostToolUse",
        &with(
            payload(&proj, "s1", "PostToolUse"),
            json!({"tool_name": "Edit", "tool_use_id": "t1",
                   "tool_input": {"file_path": proj.join("a.rs").to_string_lossy()},
                   "tool_response": {"filePath": "a.rs", "success": true}}),
        ),
    );
    assert!(post.is_none(), "{post:?}");
    // Not audited on the edit, only when the agent settles.
    assert!(e.cli(&["sentinel", "list"]).contains("no findings"));

    let stop = e.hook("Stop", &payload(&proj, "s1", "Stop")).unwrap();
    let msg = stop["systemMessage"].as_str().unwrap();
    assert!(msg.contains("Kepler Sentinel"), "{msg}");
    assert!(msg.contains("a.rs:2"), "{msg}");
    assert!(msg.contains("confirmed"), "{msg}");
    assert!(!msg.contains(&token), "the secret must never be echoed: {msg}");
    let listed = e.cli(&["sentinel", "list", "--json"]);
    assert!(listed.contains("diff.secret") && !listed.contains(&token), "{listed}");

    // The next prompt carries the labeled finding; a second prompt does not repeat it.
    let p1 = e.hook(
        "UserPromptSubmit",
        &with(payload(&proj, "s1", "UserPromptSubmit"), json!({"prompt": "now add logging to main please"})),
    );
    let c = ctx(&p1);
    assert!(c.contains("Sentinel found 1 unresolved finding(s) in the last change"), "{c}");
    assert!(c.contains("a.rs:2") && c.contains("NOT user instructions"), "{c}");
    assert!(!c.contains(&token));
    let p2 = e.hook(
        "UserPromptSubmit",
        &with(payload(&proj, "s1", "UserPromptSubmit"), json!({"prompt": "and also add a unit test for it"})),
    );
    assert!(!ctx(&p2).contains("Sentinel found"), "{}", ctx(&p2));

    // A Stop with nothing new changed audits nothing.
    assert!(e.hook("Stop", &payload(&proj, "s1", "Stop")).is_none());

    // Dismissed findings are not handed over in a new session.
    let id = serde_json::from_str::<Value>(&listed).unwrap()[0]["id"].as_i64().unwrap();
    assert!(e.cli(&["sentinel", "dismiss", &id.to_string()]).contains("dismissed"));
    let p3 = e.hook(
        "UserPromptSubmit",
        &with(payload(&proj, "s2", "UserPromptSubmit"), json!({"prompt": "continue with the refactor of the parser"})),
    );
    assert!(!ctx(&p3).contains("Sentinel found"), "{}", ctx(&p3));
}

#[test]
fn sentinel_off_and_outside_paths_are_ignored() {
    let e = Env::new();
    let proj = e.project("repo");
    git(&proj, &["init", "-q"]);
    std::fs::write(proj.join("a.rs"), "fn main() {}\n").unwrap();
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-q", "-m", "init"]);
    std::fs::write(proj.join("a.rs"), "fn main() {}\nlet x = y.unwrap();\nunsafe { z() }\n").unwrap();
    let edit = |file: &str| {
        with(
            payload(&proj, "s1", "PostToolUse"),
            json!({"tool_name": "Write", "tool_use_id": "t",
                   "tool_input": {"file_path": file}, "tool_response": {"type": "update"}}),
        )
    };
    // Outside the project and parent traversal: never audited.
    e.hook("PostToolUse", &edit("C:/Windows/system.ini"));
    e.hook("PostToolUse", &edit("../outside.rs"));
    assert!(e.hook("Stop", &payload(&proj, "s1", "Stop")).is_none());
    // KEPLER_SENTINEL=off: the edit is not tracked at all.
    e.hook_env("PostToolUse", &edit(&proj.join("a.rs").to_string_lossy()), &[("KEPLER_SENTINEL", "off")]);
    assert!(e.hook("Stop", &payload(&proj, "s1", "Stop")).is_none());
}

#[test]
fn optional_check_runs_only_when_enabled_and_after_guardian() {
    let e = Env::new();
    let proj = e.project("repo");
    git(&proj, &["init", "-q"]);
    std::fs::write(proj.join("a.rs"), "fn main() {}\n").unwrap();
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-q", "-m", "init"]);
    std::fs::write(proj.join("a.rs"), "fn main() {}\n// changed\n").unwrap();
    let edit = with(
        payload(&proj, "s1", "PostToolUse"),
        json!({"tool_name": "Edit", "tool_input": {"file_path": proj.join("a.rs").to_string_lossy()},
               "tool_response": {"success": true}}),
    );
    e.hook("PostToolUse", &edit);
    // A destructive check command is refused by Guardian and never run.
    let marker = proj.join("ran.txt");
    let stop = e
        .hook_env(
            "Stop",
            &payload(&proj, "s1", "Stop"),
            &[("KEPLER_SENTINEL_CHECK", &format!("bad=rm -rf {}", proj.to_string_lossy().replace('\\', "/")))],
        )
        .unwrap();
    assert!(
        stop["systemMessage"].as_str().unwrap().contains("was not run"),
        "{stop}"
    );
    assert!(proj.join("a.rs").exists() && !marker.exists());
}

#[test]
fn guardian_bounds_only_unbounded_reads_of_huge_files() {
    let e = Env::new();
    let proj = e.project("big");
    let big = proj.join("big.txt");
    std::fs::write(&big, "some line of text here\n".repeat(3000)).unwrap();
    let read = |extra: Value| {
        let mut ti = json!({"file_path": big.to_string_lossy()});
        for (k, v) in extra.as_object().unwrap() {
            ti[k] = v.clone();
        }
        with(
            payload(&proj, "s1", "PreToolUse"),
            json!({"tool_name": "Read", "tool_use_id": "r", "tool_input": ti}),
        )
    };
    let out = e.hook("PreToolUse", &read(json!({}))).unwrap();
    let h = &out["hookSpecificOutput"];
    assert_eq!(h["updatedInput"]["limit"], 800, "{out}");
    assert_eq!(h["updatedInput"]["file_path"], big.to_string_lossy().as_ref());
    assert!(h["additionalContext"].as_str().unwrap().contains("3000"), "{out}");
    // Already bounded by the model: untouched.
    assert!(e.hook("PreToolUse", &read(json!({"limit": 40}))).is_none());
    assert!(e.hook("PreToolUse", &read(json!({"offset": 100}))).is_none());
    // Configurable and opt-out.
    let o = e
        .hook_env("PreToolUse", &read(json!({})), &[("KEPLER_GUARDIAN_READ_MAX_LINES", "200")])
        .unwrap();
    assert_eq!(o["hookSpecificOutput"]["updatedInput"]["limit"], 200);
    assert!(e
        .hook_env("PreToolUse", &read(json!({})), &[("KEPLER_GUARDIAN_TOKENS", "off")])
        .is_none());
    // Sensitive paths are still denied by the preflight, never "optimized".
    std::fs::write(proj.join(".env"), "A=1\n".repeat(5000)).unwrap();
    let denied = e
        .hook(
            "PreToolUse",
            &with(
                payload(&proj, "s1", "PreToolUse"),
                json!({"tool_name": "Read", "tool_input": {"file_path": proj.join(".env").to_string_lossy()}}),
            ),
        )
        .unwrap();
    assert_eq!(denied["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(denied["hookSpecificOutput"].get("updatedInput").is_none());
    // The saving is recorded as an estimate.
    pause();
    let inspect = e.cli(&["inspect"]);
    assert!(inspect.contains("guardian_read_truncated_saved"), "{inspect}");
}

#[test]
fn repeated_reads_big_outputs_and_measured_usage_are_accounted() {
    let e = Env::new();
    let proj = e.project("acct");
    let f = proj.join("a.txt");
    std::fs::write(&f, "hello").unwrap();
    let read = with(
        payload(&proj, "s1", "PostToolUse"),
        json!({"tool_name": "Read", "tool_input": {"file_path": f.to_string_lossy()}, "tool_response": "hello"}),
    );
    assert!(e.hook("PostToolUse", &read).is_none());
    let second = e.hook("PostToolUse", &read).unwrap();
    assert!(
        second["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("already read"),
        "{second}"
    );
    let big = with(
        payload(&proj, "s1", "PostToolUse"),
        json!({"tool_name": "Bash", "tool_input": {"command": "cat huge"},
               "tool_response": {"stdout": "word ".repeat(30_000), "stderr": ""}}),
    );
    let o = e.hook("PostToolUse", &big).unwrap();
    assert!(ctx(&Some(o)).contains("result was large"));

    // Measured usage from the transcript, summed once per message id.
    let tr = proj.join("t.jsonl");
    let line = |id: &str, i: u64, o: u64| {
        format!("{{\"type\":\"assistant\",\"message\":{{\"id\":\"{id}\",\"usage\":{{\"input_tokens\":{i},\"output_tokens\":{o},\"cache_read_input_tokens\":1000,\"cache_creation_input_tokens\":50}}}}}}\n")
    };
    std::fs::write(&tr, format!("{}{}{}", line("a", 10, 1), line("a", 10, 5), line("b", 7, 3))).unwrap();
    e.hook(
        "Stop",
        &with(payload(&proj, "s1", "Stop"), json!({"transcript_path": tr.to_string_lossy()})),
    );
    pause();
    let v: Value = serde_json::from_str(&e.cli(&["inspect"])).unwrap();
    let rows = v["token_summary"]["by_kind"].as_array().unwrap();
    let find = |k: &str, basis: &str| {
        rows.iter()
            .find(|r| r["kind"] == k && r["basis"] == basis)
            .map(|r| r["tokens"].as_i64().unwrap())
    };
    assert_eq!(find("usage_input", "measured"), Some(17), "{rows:?}");
    assert_eq!(find("usage_output", "measured"), Some(8), "{rows:?}");
    assert!(find("guardian_big_output", "estimated").unwrap() > 6000);
    assert!(find("guardian_repeat_read", "estimated").is_some());
    assert!(find("usage_output", "estimated").is_none(), "bases are never mixed");
}
