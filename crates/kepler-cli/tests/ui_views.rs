#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! End-to-end: real hook events (through the real daemon) populate a database, and the UI's
//! module/session/forge/guardian views report them honestly, without ever showing prompt text.

use serde_json::{json, Value};
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static N: AtomicU64 = AtomicU64::new(0);

struct Env {
    home: PathBuf,
    ipc: String,
}

impl Env {
    fn new() -> Self {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let home = std::env::temp_dir().join(format!("kepler-uiv-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        Self {
            home,
            ipc: format!("uiv{}-{n}", std::process::id()),
        }
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
        c.args(args)
            .env("KEPLER_HOME", &self.home)
            .env("KEPLER_IPC_NAME", &self.ipc)
            .env("KEPLER_IDLE_TIMEOUT_SECS", "3")
            .env("KEPLER_FORGE_MODEL", "off")
            .env_remove("KEPLER_GUARDIAN_RULES")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        c
    }

    fn hook(&self, event: &str, payload: Value) {
        let mut child = self.cmd(&["hook", "claude-code", event]).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{event}");
    }
}

/// Stops the UI child even when an assertion fails.
struct Kill(std::process::Child);

impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn http(port: u16, token: &str, path: &str) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = if token.is_empty() {
        String::new()
    } else {
        format!("Authorization: Bearer {token}\r\n")
    };
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}\r\n").as_bytes())
        .unwrap();
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    let status = buf.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = buf.split_once("\r\n\r\n").map_or("", |x| x.1).to_string();
    (status, body)
}

#[test]
fn views_report_real_hook_activity_honestly_and_never_show_prompt_text() {
    let env = Env::new();
    let (s, cwd) = ("sess-e2e", "C:\\work\\proj");
    let base = |ev: &str| json!({"session_id": s, "cwd": cwd, "hook_event_name": ev});
    let with = |ev: &str, extra: Value| {
        let mut b = base(ev);
        b.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        b
    };
    env.hook(
        "SessionStart",
        with("SessionStart", json!({"source": "startup"})),
    );
    env.hook(
        "UserPromptSubmit",
        with(
            "UserPromptSubmit",
            json!({"prompt": "fix the bug in the login flow please"}),
        ),
    );
    env.hook(
        "UserPromptSubmit",
        with(
            "UserPromptSubmit",
            json!({"prompt": "why is the build slow?"}),
        ),
    );
    env.hook(
        "PreToolUse",
        with(
            "PreToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf /"}, "tool_use_id": "t1"}),
        ),
    );
    env.hook(
        "PostToolUse",
        with(
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_input": {"command": "env"}, "tool_use_id": "t2",
                "tool_response": {"stdout": "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n", "stderr": ""}}),
        ),
    );
    env.hook("Stop", with("Stop", json!({"stop_hook_active": false})));

    let mut ui = Kill(
        env.cmd(&["ui", "--json"])
            .stdin(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    std::io::BufReader::new(ui.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    let port = v["port"].as_u64().unwrap() as u16;
    let token = v["token"].as_str().unwrap().to_string();
    let get = |p: &str| -> Value {
        let (st, body) = http(port, &token, p);
        assert_eq!(st, 200, "{p}: {body}");
        serde_json::from_str(&body).unwrap()
    };

    // The daemon writes asynchronously: wait for the six metadata rows.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let t = get("/api/summary")["events"]["total"].as_i64().unwrap();
        if t >= 6 {
            break;
        }
        assert!(Instant::now() < deadline, "events never arrived: {t}");
        std::thread::sleep(Duration::from_millis(200));
    }

    let mut all = String::new();
    let mut take = |p: &str| {
        let v = get(p);
        all.push_str(&v.to_string());
        v
    };
    let sum = take("/api/summary");
    assert_eq!(sum["harnesses"][0]["harness"], "claude-code");
    assert_eq!(
        sum["database"]["schema_version"],
        sum["database"]["latest_schema"]
    );

    let mods = take("/api/modules");
    let m = |id: &str| -> Value {
        mods["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(mods["modules"].as_array().unwrap().len(), 8);
    assert_eq!(m("forge")["state"], "active");
    assert_eq!(m("guardian")["state"], "active");
    assert!(m("guardian")["limitation"]
        .as_str()
        .unwrap()
        .contains("NO esta implementada"));
    assert_eq!(m("genesis")["state"], "not_implemented");
    assert_eq!(m("orbit")["state"], "none", "no runs, no activity invented");
    assert_eq!(m("vision")["state"], "none");

    let forge = take("/api/forge");
    assert!(forge["classified_total"].as_i64().unwrap() >= 2);
    assert!(forge["modes"]["enrich"].as_i64().unwrap() >= 1, "{forge}");
    assert_eq!(forge["model"]["available"], false);

    let g = take("/api/guardian");
    let reasons = g["reasons"].to_string();
    assert!(reasons.contains("destructive_command"), "{reasons}");
    assert!(g["top_tools"].to_string().contains("Bash"), "{g}");

    let sessions = take("/api/sessions");
    assert_eq!(sessions["total"], 1);
    assert_eq!(sessions["sessions"][0]["prompts"], 2);
    let sid = sessions["sessions"][0]["session_id"].as_str().unwrap();
    let detail = take(&format!("/api/session?id={sid}"));
    assert!(detail["timeline"].as_array().unwrap().len() >= 6);

    let ev = take("/api/events?decision=require_approval&limit=5");
    assert!(ev["total"].as_i64().unwrap() >= 1);
    let flt = take("/api/events?module=forge&window=1h");
    assert!(flt["total"].as_i64().unwrap() >= 1);
    for p in [
        "/api/memory",
        "/api/sentinel",
        "/api/vision-focus",
        "/api/orbit",
    ] {
        take(p);
    }

    // Privacy: nothing that was typed or printed shows up in any view.
    for secret in [
        "login flow",
        "build slow",
        "AKIAIOSFODNN7EXAMPLE",
        "rm -rf",
        "C:\\\\work\\\\proj",
    ] {
        assert!(!all.contains(secret), "leaked {secret}");
    }
    assert_eq!(http(port, "", "/api/modules").0, 401);
}
