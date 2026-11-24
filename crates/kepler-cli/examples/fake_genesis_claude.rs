#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Test double for `claude -p` as used by `kepler genesis design` (point `KEPLER_CLAUDE_BIN` at
//! it). Not shipped: a cargo example used by the integration tests.
//!
//! `FAKE_CLAUDE_MODE`: ok (default) | garbage | hostile | slow | fail | recurse | huge
//! `FAKE_CLAUDE_LOG=<file>` appends one JSON line per invocation (args, guard variables, cwd,
//! stdin) so tests can assert what Kepler sent and how it launched the child.

use serde_json::{json, Value};
use std::io::{Read, Write};

fn candidate(n: usize) -> Value {
    json!({
        "name": format!("Candidate {n}"),
        "rationale": "Focused layout that reuses the project's primary action color.",
        "goal": "Let people sign in, register or recover a password.",
        "audience": ["returning customers"],
        "screens": [{
            "id": "login", "name": "Sign in", "purpose": "Authenticate an existing user.",
            "layout": {"kind": "centered_card", "description": "Card centered on the page."},
            "sections": [{"id": "form", "name": "Credentials", "components": [
                {"kind": "heading", "label": "Welcome back"},
                {"kind": "email_input", "label": "Email", "library_component": "Input"},
                {"kind": "password_input", "label": "Password", "library_component": "Input"},
                {"kind": "button", "label": "Sign in", "library_component": "Button"}
            ]}],
            "states": {"empty": "Placeholders.", "loading": "Spinner on the button.", "error": "Inline alert."},
            "responsive": [{"breakpoint": "mobile", "behavior": "Full width card."}],
            "accessibility": ["Labels bound to inputs"]
        }],
        "tokens_used": [{"role": "primary action", "token": "brand"}],
        "implementation_plan": [{"path": "src/pages/Login.tsx", "action": "create", "description": "Login screen."}]
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    if let Ok(path) = std::env::var("FAKE_CLAUDE_LOG") {
        let line = json!({
            "args": args,
            "guard": std::env::var("KEPLER_FORGE_CHILD").ok(),
            "genesis_guard": std::env::var("KEPLER_GENESIS_CHILD").ok(),
            "forge_env": std::env::var("KEPLER_FORGE").ok(),
            "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().to_string()),
            "stdin": stdin,
        });
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
    let n: usize = stdin
        .split("Return ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|d| d.parse().ok())
        .unwrap_or(1);
    let valid =
        json!({"schema_version": 1, "candidates": (1..=n).map(candidate).collect::<Vec<_>>()});
    let mode = std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "ok".into());
    let result = match mode.as_str() {
        "fail" => {
            eprintln!("Not logged in");
            std::process::exit(3);
        }
        "slow" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            valid.to_string()
        }
        "garbage" => "Sorry, I cannot help with that.".to_string(),
        "hostile" => {
            let mut c = candidate(1);
            c["name"] = json!("<script>alert(1)</script>Pwned");
            c["screens"][0]["sections"][0]["components"][0]["label"] =
                json!("<img src=x onerror=alert(1)>Hi");
            let mut bad = candidate(2);
            bad["run"] = json!("rm -rf /");
            json!({"schema_version": 1, "candidates": [c, bad]}).to_string()
        }
        "huge" => "x".repeat(200_000),
        "recurse" => {
            // What a real `claude -p` does with the user's global hooks: run the Kepler hook,
            // inheriting our environment.
            if let Ok(exe) = std::env::var("FAKE_CLAUDE_HOOK_EXE") {
                let payload = json!({
                    "session_id": "child", "cwd": ".", "hook_event_name": "UserPromptSubmit",
                    "prompt": "Elimina ese gk de todos lados"
                })
                .to_string();
                let mut child = std::process::Command::new(&exe)
                    .args(["hook", "claude-code", "UserPromptSubmit"])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn hook");
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(payload.as_bytes())
                    .unwrap();
                let out = child.wait_with_output().unwrap();
                // And what a nested `kepler genesis design` would do: it must refuse.
                let nested = std::process::Command::new(&exe)
                    .args(["genesis", "design", "nested brief"])
                    .stdin(std::process::Stdio::null())
                    .output()
                    .unwrap();
                if let Ok(path) = std::env::var("FAKE_CLAUDE_LOG") {
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .unwrap();
                    writeln!(
                        f,
                        "{}",
                        json!({"nested_hook_exit": out.status.code(), "nested_hook_stdout_bytes": out.stdout.len(),
                               "nested_genesis_exit": nested.status.code()})
                    )
                    .unwrap();
                }
            }
            valid.to_string()
        }
        _ => valid.to_string(),
    };
    let out = json!({
        "type": "result", "subtype": "success", "is_error": false, "result": result,
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": 5, "cache_creation_input_tokens": 900, "cache_read_input_tokens": 0, "output_tokens": 800},
    });
    println!("{out}");
}
