#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Test double for the sub-agent that Orbit's `agent` tasks run (`claude -p <prompt> ...`).
//! Point `KEPLER_AGENT_BIN` at it. Not shipped: it is a cargo example used by the integration
//! tests and by `scripts/acceptance.ps1`. It never calls a model.
//!
//! The prompt is the instruction:
//!   `write <file> <text...>`   create `<file>` (relative to the worktree) with `<text>`
//!   `integrate`                write `integrated.txt` listing the files present in the worktree
//!                              (proves the dependencies' branches were merged into it)
//!   `fail`                     exit 7
//!   `chatty`                   print ~100 KiB, then a secret-shaped token
//! Anything else just prints "ok". The argv it received is written to `<FAKE_AGENT_LOG>.<pid>`.

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // One file per invocation (`<log>.<pid>`): parallel agents never interleave their lines.
    if let Ok(path) = std::env::var("FAKE_AGENT_LOG") {
        if let Ok(mut f) = std::fs::File::create(format!("{path}.{}", std::process::id())) {
            let _ = writeln!(f, "{}", serde_json::json!(args));
        }
    }
    assert_eq!(
        args.first().map(String::as_str),
        Some("-p"),
        "expected -p <prompt>"
    );
    let prompt = args.get(1).cloned().unwrap_or_default();
    let mut words = prompt.splitn(3, ' ');
    match words.next().unwrap_or("") {
        "write" => {
            let file = words.next().unwrap();
            let text = words.next().unwrap_or("");
            std::fs::write(file, format!("{text}\n")).unwrap();
            println!("wrote {file}");
        }
        "integrate" => {
            let mut names: Vec<String> = std::fs::read_dir(".")
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .collect();
            names.sort();
            std::fs::write("integrated.txt", names.join("\n")).unwrap();
            println!("integrated {}", names.len());
        }
        "fail" => {
            eprintln!("agent failed on purpose");
            std::process::exit(7);
        }
        "chatty" => {
            for i in 0..2000 {
                println!("line {i:05} ................................................");
            }
            println!("token ghp_{}", "a1B2".repeat(9));
        }
        _ => println!("ok"),
    }
}
