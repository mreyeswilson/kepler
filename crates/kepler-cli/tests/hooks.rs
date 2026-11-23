//! End-to-end tests for `kepler hook claude-code`: stdin payload -> real daemon -> stdout.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

struct Env {
    home: PathBuf,
    ipc: String,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let home =
            std::env::temp_dir().join(format!("kepler-hook-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            home,
            ipc: format!("hk{}-{n}", std::process::id()),
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
        c.args(args)
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_IPC_NAME", &self.ipc)
            .env("KEPLER_IDLE_TIMEOUT_SECS", "3")
            .env_remove("KEPLER_GUARDIAN_RULES")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn run_with(&self, mut c: Command, stdin: &[u8]) -> Output {
        let mut child = c.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }

    /// Run `kepler hook claude-code <event> [extra]` with stdin; returns (exit code, stdout JSON).
    fn hook(&self, event: &str, stdin: &str, extra: &[&str]) -> (i32, Option<Value>) {
        let mut args = vec!["hook", "claude-code", event];
        args.extend_from_slice(extra);
        let o = self.run_with(self.cmd(&args), stdin.as_bytes());
        let out = String::from_utf8_lossy(&o.stdout);
        let json = (!out.trim().is_empty())
            .then(|| serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}")));
        (o.status.code().unwrap(), json)
    }

    fn pre_bash(&self, command: &str) -> (i32, Option<Value>) {
        let p = json!({
            "session_id": "sess 1/../x", "cwd": "C:\\work\\proj", "hook_event_name": "PreToolUse",
            "tool_name": "Bash", "tool_input": {"command": command}, "tool_use_id": "toolu_1"
        });
        self.hook("PreToolUse", &p.to_string(), &[])
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Let the short-lived daemon notice; best effort cleanup of the data dir.
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn decision(v: &Option<Value>) -> &str {
    v.as_ref().unwrap()["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .unwrap()
}

#[test]
fn pre_tool_use_destructive_commands_ask_by_default() {
    let e = Env::new();
    for cmd in ["rm -rf /", "git push --force origin main"] {
        let (code, out) = e.pre_bash(cmd);
        assert_eq!(code, 0, "{cmd}");
        assert_eq!(decision(&out), "ask", "{cmd}");
        assert_eq!(
            out.unwrap()["hookSpecificOutput"]["hookEventName"],
            "PreToolUse"
        );
    }
}

#[test]
fn pre_tool_use_user_deny_rules_and_sensitive_paths_deny() {
    let e = Env::new();
    let rules = e.home.join("rules.txt");
    std::fs::write(
        &rules,
        "deny-command rm -rf /\ndeny-command git push --force\n",
    )
    .unwrap();
    for cmd in ["rm -rf /", "git push --force origin main"] {
        let mut c = e.cmd(&["hook", "claude-code", "PreToolUse"]);
        c.env("KEPLER_GUARDIAN_RULES", &rules);
        let p =
            json!({"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{"command":cmd}});
        let o = e.run_with(c, p.to_string().as_bytes());
        assert_eq!(o.status.code(), Some(0));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["permissionDecision"], "deny",
            "{cmd}"
        );
        assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .starts_with("Kepler blocked"));
    }
    // Built-in: sensitive path is denied with no rules at all.
    let p = json!({"session_id":"s","cwd":"/p","tool_name":"Write",
        "tool_input":{"file_path":"/p/.env","content":"K=V"}});
    let (code, out) = e.hook("PreToolUse", &p.to_string(), &[]);
    assert_eq!(code, 0);
    assert_eq!(decision(&out), "deny");
}

#[test]
fn pre_tool_use_harmless_command_is_silent_passthrough() {
    let e = Env::new();
    let (code, out) = e.pre_bash("cargo test");
    assert_eq!(code, 0);
    assert!(out.is_none(), "{out:?}");
}

#[test]
fn post_tool_use_secret_leak_warns_and_never_echoes_secret() {
    let e = Env::new();
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let p = json!({"session_id":"s","cwd":"/p","hook_event_name":"PostToolUse","tool_name":"Bash",
        "tool_input":{"command":"env"},
        "tool_response":{"stdout": format!("AWS_ACCESS_KEY_ID={secret}\n"),"stderr":"","interrupted":false},
        "tool_use_id":"t1"});
    let (code, out) = e.hook("PostToolUse", &p.to_string(), &[]);
    assert_eq!(code, 0);
    let out = out.expect("systemMessage expected");
    assert!(out["systemMessage"]
        .as_str()
        .unwrap()
        .contains("aws_access_key"));
    assert!(!out.to_string().contains(secret));
    assert!(out.get("decision").is_none() && out.get("hookSpecificOutput").is_none());
    // Nothing persisted: inspect shows metadata only.
    let o = e.run_with(e.cmd(&["inspect"]), b"");
    assert!(!String::from_utf8_lossy(&o.stdout).contains(secret));
    // A clean response stays silent.
    let clean = json!({"session_id":"s","cwd":"/p","tool_name":"Bash",
        "tool_response":{"stdout":"hello","stderr":""}});
    assert!(e.hook("PostToolUse", &clean.to_string(), &[]).1.is_none());
}

#[test]
fn user_prompt_submit_adds_labeled_context_only() {
    let e = Env::new();
    let prompt = "Refactor the parser, keep tests green.";
    let p =
        json!({"session_id":"s","cwd":"/p","hook_event_name":"UserPromptSubmit","prompt":prompt});
    let (code, out) = e.hook("UserPromptSubmit", &p.to_string(), &[]);
    assert_eq!(code, 0);
    let out = out.expect("a vague refactor prompt is enriched by Forge");
    let h = &out["hookSpecificOutput"];
    assert_eq!(h["hookEventName"], "UserPromptSubmit");
    let c = h["additionalContext"].as_str().unwrap();
    assert!(c.contains("NOT user instructions"));
    // The prompt is never echoed back or rewritten.
    assert!(!c.contains(prompt));
    assert!(out.get("decision").is_none() && out.get("continue").is_none());
}

#[test]
fn lifecycle_events_are_accepted_and_silent() {
    let e = Env::new();
    for (ev, extra) in [
        ("SessionStart", json!({"source":"startup"})),
        ("Stop", json!({"stop_hook_active": false})),
        ("SessionEnd", json!({"reason":"clear"})),
    ] {
        let mut p = json!({"session_id":"s","cwd":"/p","hook_event_name":ev});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let (code, out) = e.hook(ev, &p.to_string(), &[]);
        assert_eq!(code, 0, "{ev}");
        assert!(out.is_none(), "{ev}: {out:?}");
    }
    // The daemon recorded them (metadata only).
    let o = e.run_with(e.cmd(&["inspect"]), b"");
    let s = String::from_utf8_lossy(&o.stdout).to_string();
    for k in ["agent.settled", "session.closed", "agent.started"] {
        assert!(s.contains(k), "missing {k} in {s}");
    }
}

#[test]
fn core_down_is_passthrough_exit_0() {
    let e = Env::new();
    let p =
        json!({"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{"command":"rm -rf /"}});
    let (code, out) = e.hook("PreToolUse", &p.to_string(), &["--no-autostart"]);
    assert_eq!(code, 0);
    assert!(out.is_none(), "no core => no verdict => no output: {out:?}");
}

#[test]
fn malformed_and_odd_input_never_breaks_claude_code() {
    let e = Env::new();
    for stdin in [
        "",
        "not json",
        "{",
        "[]",
        "null",
        "{\"prompt\": 5}",
        "\u{0}\u{1}",
    ] {
        for ev in [
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "Stop",
            "SessionEnd",
        ] {
            let (code, out) = e.hook(ev, stdin, &["--no-autostart"]);
            assert_eq!(code, 0, "{ev} {stdin:?}");
            assert!(out.is_none());
        }
    }
    // Invalid UTF-8 bytes.
    let o = e.run_with(
        e.cmd(&["hook", "claude-code", "PreToolUse", "--no-autostart"]),
        &[0xff, 0xfe, 0x00],
    );
    assert_eq!(o.status.code(), Some(0));
    // Unknown event / missing event are ignored too.
    assert_eq!(e.hook("FutureEvent", "{}", &[]).0, 0);
    let o = e.run_with(e.cmd(&["hook", "claude-code"]), b"{}");
    assert_eq!(o.status.code(), Some(0));
}

#[test]
fn timeout_is_passthrough() {
    let e = Env::new();
    let p =
        json!({"session_id":"s","cwd":"/p","tool_name":"Bash","tool_input":{"command":"rm -rf /"}});
    // 1 ms cannot cover connecting to a daemon that has to be autostarted.
    let mut c = e.cmd(&["hook", "claude-code", "PreToolUse"]);
    c.env("KEPLER_HOOK_TIMEOUT_MS", "1");
    let o = e.run_with(c, p.to_string().as_bytes());
    assert_eq!(o.status.code(), Some(0));
    assert!(
        o.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
}

#[test]
fn print_settings_is_pasteable_json_with_absolute_exe() {
    let e = Env::new();
    for shell_form in [false, true] {
        let mut args = vec!["hook", "claude-code", "--print-settings"];
        if shell_form {
            args.push("--shell-form");
        }
        let o = e.run_with(e.cmd(&args), b"");
        assert_eq!(o.status.code(), Some(0));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 6);
        let h = &hooks["PreToolUse"][0]["hooks"][0];
        assert_eq!(
            hooks["PreToolUse"][0]["matcher"],
            "Bash|Write|Edit|MultiEdit|NotebookEdit|Read"
        );
        assert!(h["timeout"].as_u64().unwrap() <= 5);
        let cmd = h["command"].as_str().unwrap();
        if shell_form {
            assert!(cmd.starts_with('\'') && cmd.ends_with("hook claude-code PreToolUse"));
        } else {
            let exe = PathBuf::from(cmd);
            assert!(exe.is_absolute(), "{cmd}");
            assert_eq!(h["args"], json!(["hook", "claude-code", "PreToolUse"]));
        }
    }
}
