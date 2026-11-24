//! REAL `claude -p` run of `kepler genesis design` (ignored by default: it calls Anthropic with
//! the user's own login and opens Edge/Chrome headless).
//!
//! `cargo test -p kepler-cli --test genesis_real -- --ignored --nocapture`
//! Env: `KEPLER_GENESIS_MODEL` (default: Genesis' default), `GENESIS_REAL_OUT=<dir>` to keep the
//! generated design (default: a temp dir that is kept so the PNG can be inspected).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[test]
#[ignore = "calls the real claude CLI (network, billed to the user's login)"]
fn real_login_design_has_a_valid_schema_and_renders() {
    let out: PathBuf = std::env::var_os("GENESIS_REAL_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("kepler-genesis-real-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/genesis-react");
    let t = Instant::now();
    let o = Command::new(env!("CARGO_BIN_EXE_kepler"))
        .args([
            "genesis",
            "design",
            "pantalla de login con registro y recuperación de contraseña",
            "--project",
            project.to_str().unwrap(),
            "--candidates",
            "2",
            "--json",
        ])
        .env("KEPLER_HOME", &out)
        .env_remove("KEPLER_FORGE_CHILD")
        .env_remove("KEPLER_GENESIS_CHILD")
        .output()
        .unwrap();
    let wall = t.elapsed();
    assert!(o.status.success(), "stderr: {}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let cands = v["candidates"].as_array().unwrap();
    println!("model={} wall={:?} model_latency_ms={} usage={}", v["model"], wall, v["latency_ms"], v["usage"]);
    println!("candidates={} rejected={}", cands.len(), v["rejected_candidates"]);
    assert!(!cands.is_empty());
    let dir = PathBuf::from(v["directory"].as_str().unwrap());
    let spec: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("spec.json")).unwrap()).unwrap();
    let mut mapped = 0;
    let mut total = 0;
    for c in spec["candidates"].as_array().unwrap() {
        for s in c["screens"].as_array().unwrap() {
            for sec in s["sections"].as_array().unwrap() {
                for comp in sec["components"].as_array().unwrap() {
                    total += 1;
                    if comp["library_component"].is_string() {
                        mapped += 1;
                    }
                }
            }
        }
        println!(
            "  - {}: {} screen(s), plan files {}, unknown tokens {}",
            c["name"],
            c["screens"].as_array().unwrap().len(),
            c["implementation_plan"].as_array().unwrap().len(),
            c["unknown_tokens"]
        );
    }
    println!("components mapped to the project's library: {mapped}/{total}");
    for (i, c) in cands.iter().enumerate() {
        let html = std::fs::read_to_string(c["preview_html"].as_str().unwrap()).unwrap().to_lowercase();
        assert!(!html.contains("<script") && html.contains("--primary:#4f46e5"));
        println!("  preview {}: html={} png={}", i + 1, c["preview_html"], c["preview_png"]);
    }
    println!("capture={}", v["capture"]);
    println!("kept in {}", out.display());
}
