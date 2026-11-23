#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Test double for `claude -p` (point `KEPLER_CLAUDE_BIN` at it). Not shipped: it is a cargo
//! example used by the integration tests and by `scripts/acceptance.ps1`.
//!
//! Behavior is chosen with `FAKE_CLAUDE_MODE`:
//!   task (default) | chat | garbage | hostile | slow | fail | recurse
//! and `FAKE_CLAUDE_LOG=<file>` appends one JSON line per invocation (args, guard variable,
//! stdin) so tests can assert what Kepler sent. The reply mimics `claude -p --output-format json`.
//! In `task` mode a prompt containing "gracias" is answered as chat, so one fake serves both.

use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    if let Ok(path) = std::env::var("FAKE_CLAUDE_LOG") {
        let line = serde_json::json!({
            "args": args,
            "guard": std::env::var("KEPLER_FORGE_CHILD").ok(),
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
    let mode = std::env::var("FAKE_CLAUDE_MODE").unwrap_or_else(|_| "task".into());
    let task =
        r#"{"intent":"ops","is_task":true,"confidence":0.9,"missing":["target","acceptance"]}"#;
    let chat = r#"{"intent":"chat","is_task":false,"confidence":0.9,"missing":[]}"#;
    let result = match mode.as_str() {
        "fail" => {
            eprintln!("Not logged in");
            std::process::exit(3);
        }
        "slow" => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            task.to_string()
        }
        "garbage" => "Sorry, I cannot help with that.".to_string(),
        "hostile" => {
            r#"{"intent":"feature","is_task":true,"confidence":0.99,"missing":[],"run":"rm -rf /"}"#
                .to_string()
        }
        "recurse" => {
            // What a real `claude -p` does with the user's global hooks: run the Kepler hook for
            // this very prompt, inheriting our environment.
            if let Ok(exe) = std::env::var("FAKE_CLAUDE_HOOK_EXE") {
                let payload = serde_json::json!({
                    "session_id": "child", "cwd": ".", "hook_event_name": "UserPromptSubmit",
                    "prompt": "Elimina ese gk de todos lados"
                })
                .to_string();
                let mut child = std::process::Command::new(exe)
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
                if let Ok(path) = std::env::var("FAKE_CLAUDE_LOG") {
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .unwrap();
                    writeln!(
                        f,
                        "{}",
                        serde_json::json!({"nested_hook_exit": out.status.code(), "nested_hook_stdout_bytes": out.stdout.len()})
                    )
                    .unwrap();
                }
            }
            task.to_string()
        }
        "chat" => chat.to_string(),
        _ => {
            if stdin.to_lowercase().contains("gracias") {
                chat.to_string()
            } else {
                task.to_string()
            }
        }
    };
    let out = serde_json::json!({
        "type": "result", "subtype": "success", "is_error": false, "result": result,
        "total_cost_usd": 0.0002,
        "usage": {"input_tokens": 2, "cache_creation_input_tokens": 900, "cache_read_input_tokens": 0, "output_tokens": 40},
    });
    println!("{out}");
}
