//! Simple, honest measurements (no statistics framework). Run with:
//!   cargo run --release -p kepler-core --example bench
//! Prints p50/p95/p99 in microseconds for (1) in-process dispatch and (2) one IPC round trip
//! through a real daemon on a named pipe / Unix socket, over an already-open connection.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use kepler_core::daemon::{Daemon, DaemonConfig};
use kepler_core::{build_engine, PipelineConfig};
use kepler_events::Envelope;
use kepler_ipc::{Client, Endpoint, HelloOptions};
use kepler_memory::MemoryAdmin;
use kepler_storage::{Scope, Writer, WriterConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn event(id: usize, kind: &str, payload: Value) -> Value {
    json!({
        "version": 1, "id": format!("b{id}"), "type": kind,
        "timestamp": "2026-01-02T03:04:05Z", "harness": "pi",
        "project_id": "bench", "session_id": "bench-s", "correlation_id": "c",
        "payload": payload
    })
}

fn report(name: &str, mut samples: Vec<Duration>) {
    samples.sort();
    let at = |p: f64| samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)];
    println!(
        "{name:<44} n={:<5} p50={:>7.1} us  p95={:>7.1} us  p99={:>7.1} us  max={:>8.1} us",
        samples.len(),
        at(0.50).as_secs_f64() * 1e6,
        at(0.95).as_secs_f64() * 1e6,
        at(0.99).as_secs_f64() * 1e6,
        samples.last().unwrap().as_secs_f64() * 1e6
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let dir = std::env::temp_dir().join(format!("kepler-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("kepler.db");

    // Seed 500 memories so retrieval is not trivially empty.
    let admin = MemoryAdmin::new(db.clone());
    for i in 0..500 {
        admin
            .add(
                "bench",
                Scope::Project,
                "",
                &format!(
                    "note {i}: module{} handles feature{} with config{} and cache{}",
                    i % 37,
                    i,
                    i % 11,
                    i % 5
                ),
            )
            .unwrap();
    }

    // 1) in-process dispatch
    let writer = Arc::new(Writer::start(&db, WriterConfig::default()).unwrap());
    let engine = build_engine(
        &PipelineConfig {
            focus: std::env::var_os("BENCH_NO_FOCUS").is_none(),
            sentinel: std::env::var_os("BENCH_NO_SENTINEL").is_none(),
            ..PipelineConfig::default()
        },
        Some((writer.clone(), db.clone())),
    );
    let parse = |v: Value| Envelope::from_value(v).unwrap();
    let cases: Vec<(&str, Envelope)> = vec![
        (
            "trivial input (ok)",
            parse(event(1, "input.received", json!({"text": "ok"}))),
        ),
        (
            "specific input (forge passthrough + memory FTS)",
            parse(event(
                2,
                "input.received",
                json!({"text": "Fix the failing test in parser.rs for module12 feature12"}),
            )),
        ),
        (
            "vague input (forge enrich + memory + guardian)",
            parse(event(
                3,
                "input.received",
                json!({"text": "fix the bug in module5 config5 handling please"}),
            )),
        ),
        (
            "tool preflight (clean)",
            parse(event(
                4,
                "tool.call.requested",
                json!({"tool":"bash","command":"cargo test"}),
            )),
        ),
        (
            "tool result scan (4 KiB clean excerpt)",
            parse(event(
                5,
                "tool.call.completed",
                json!({"tool":"bash","ok":true,"output_bytes":4096,"duration_ms":1,"output_excerpt":"test result: ok. ".repeat(240)}),
            )),
        ),
    ];
    for (name, ev) in &cases {
        for _ in 0..200 {
            std::hint::black_box(engine.evaluate(ev)); // warm up
        }
        let mut s = Vec::new();
        for _ in 0..3000 {
            let t = Instant::now();
            std::hint::black_box(engine.evaluate(ev));
            s.push(t.elapsed());
        }
        report(&format!("in-process: {name}"), s);
    }
    drop(engine);
    drop(writer);

    // 2) IPC round trip through a real daemon
    let ep = Endpoint::new(&format!("bench{}", std::process::id())).unwrap();
    let mut cfg = DaemonConfig::new(ep.clone());
    cfg.db_path = Some(db.clone());
    cfg.idle_shutdown = None;
    let daemon = Daemon::start(cfg).unwrap();
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(daemon.serve(async move {
        let _ = rx.await;
    }));
    let mut c = Client::connect(&ep, HelloOptions::new("bench"))
        .await
        .unwrap();
    for (name, payload_kind, payload) in [
        (
            "ipc: tool preflight round trip",
            "tool.call.requested",
            json!({"tool":"bash","command":"cargo test"}),
        ),
        (
            "ipc: specific input round trip",
            "input.received",
            json!({"text":"Fix the failing test in parser.rs for module12 feature12"}),
        ),
        (
            "ipc: vague input round trip (enrich)",
            "input.received",
            json!({"text":"fix the bug in module5 config5 handling please"}),
        ),
    ] {
        for i in 0..100 {
            c.send_event_value(event(10_000 + i, payload_kind, payload.clone()))
                .await
                .unwrap();
        }
        let mut s = Vec::new();
        for i in 0..2000 {
            let ev = event(20_000 + i, payload_kind, payload.clone());
            let t = Instant::now();
            c.send_event_value(ev).await.unwrap();
            s.push(t.elapsed());
        }
        report(name, s);
    }
    drop(c);
    let _ = stop.send(());
    let _ = task.await;
    let _ = std::fs::remove_dir_all(&dir);
}
