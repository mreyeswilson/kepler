//! Real-model measurement (ADR-0026). `#[ignore]`d: it calls the user's installed `claude -p`
//! (Haiku) over the model-eligible rows of `fixtures/forge/prompts.tsv` and reports latency
//! (p50/p95), accuracy against the human labels, and the cost the CLI reports.
//!
//!   cargo test -p kepler-cli --test forge_model_real -- --ignored --nocapture
//!
//! Optional: `KEPLER_REAL_PASSES=2` (default 2) repeats each prompt with a cold cache.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kepler_forge::{analyze, Confidence};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

const CORPUS: &str = include_str!("../../../fixtures/forge/prompts.tsv");

fn pct(v: &[u128], p: f64) -> u128 {
    let mut s = v.to_vec();
    s.sort_unstable();
    s[(((s.len() - 1) as f64) * p).round() as usize]
}

#[test]
#[ignore = "calls the real claude CLI and spends a few cents of the user's own usage"]
fn real_haiku_over_the_doubtful_corpus() {
    let home = std::env::temp_dir().join(format!("kepler-real-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let rows: Vec<(String, String)> = CORPUS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (c.len() == 3).then(|| (c[1].to_string(), c[2].to_string()))
        })
        .filter(|(t, _)| analyze(t).confidence == Confidence::Doubtful)
        .collect();
    let passes: usize = std::env::var("KEPLER_REAL_PASSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let (mut lat, mut right, mut total, mut failed) = (Vec::<u128>::new(), 0usize, 0usize, 0usize);
    let mut misses = Vec::new();
    let state = home.join("forge-model.json");
    for pass in 0..passes {
        for (text, label) in &rows {
            let _ = std::fs::remove_file(&state); // cold cache, closed breaker
            let p = json!({"session_id":"s","cwd":"/p","hook_event_name":"UserPromptSubmit","prompt":text});
            let mut c = Command::new(env!("CARGO_BIN_EXE_kepler"));
            c.args(["hook", "claude-code", "UserPromptSubmit", "--no-autostart"])
                .env("KEPLER_HOME", &home)
                .env_remove("KEPLER_CLAUDE_BIN")
                .env_remove("KEPLER_FORGE_MODEL")
                .env("KEPLER_FORGE_MODEL_TIMEOUT_MS", "20000")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let t = Instant::now();
            let mut ch = c.spawn().unwrap();
            ch.stdin
                .take()
                .unwrap()
                .write_all(p.to_string().as_bytes())
                .unwrap();
            let o = ch.wait_with_output().unwrap();
            let ms = t.elapsed().as_millis();
            let stored: Value = std::fs::read(&state)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or(Value::Null);
            if stored["ok"].as_u64() != Some(1) {
                failed += 1;
                eprintln!(
                    "pass {pass}: CALL FAILED ({ms} ms) {text:?} last_error={}",
                    stored["last_error"]
                );
                continue;
            }
            lat.push(ms);
            let said_task = !String::from_utf8_lossy(&o.stdout).trim().is_empty();
            total += 1;
            if said_task == (label == "task") {
                right += 1;
            } else {
                misses.push(format!(
                    "label={label} model_said_task={said_task}: {text:?}"
                ));
            }
            eprintln!("pass {pass}: {ms:5} ms  label={label:8} said_task={said_task:5}  {text}");
            // Tokens/cost as reported by the CLI are accumulated in the state file by the hook.
            eprintln!(
                "          tokens in/out {}/{}  cost ${:.5}",
                stored["tokens_in"],
                stored["tokens_out"],
                stored["cost_usd"].as_f64().unwrap_or(0.0)
            );
        }
    }
    eprintln!("---- REAL RESULT ----");
    eprintln!(
        "doubtful prompts: {} x {passes} passes; ok calls {total}, failed {failed}",
        rows.len()
    );
    if !lat.is_empty() {
        eprintln!(
            "latency ms: p50 {}  p95 {}  max {}",
            pct(&lat, 0.5),
            pct(&lat, 0.95),
            pct(&lat, 1.0)
        );
    }
    eprintln!("accuracy (task vs not-task): {right}/{total}");
    for m in &misses {
        eprintln!("MISS {m}");
    }
    let _ = std::fs::remove_dir_all(&home);
}
