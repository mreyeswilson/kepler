#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The documented pipeline through the real daemon: original -> Forge -> Memory (-> Guardian).

use kepler_core::daemon::{Daemon, DaemonConfig};
use kepler_ipc::{Client, Endpoint, HelloOptions, ServeExit};
use kepler_memory::MemoryAdmin;
use kepler_policy::Decision;
use kepler_storage::MemoryFilter;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

static N: AtomicU64 = AtomicU64::new(0);

struct Running {
    ep: Endpoint,
    db: PathBuf,
    task: JoinHandle<ServeExit>,
    stop: tokio::sync::oneshot::Sender<()>,
}

fn start() -> Running {
    start_with(|_| {})
}

fn start_with(tweak: impl FnOnce(&mut DaemonConfig)) -> Running {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let ep = Endpoint::new(&format!("pipe{}-{n}", std::process::id())).unwrap();
    let dir = std::env::temp_dir().join(format!("kepler-pipe-e2e-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("kepler.db");
    let mut cfg = DaemonConfig::new(ep.clone());
    cfg.db_path = Some(db.clone());
    cfg.idle_shutdown = None;
    tweak(&mut cfg);
    let daemon = Daemon::start(cfg).unwrap();
    assert!(daemon.warnings.is_empty(), "{:?}", daemon.warnings);
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(daemon.serve(async move {
        let _ = rx.await;
    }));
    Running { ep, db, task, stop }
}

fn event(id: &str, session: &str, kind: &str, payload: Value) -> Value {
    json!({
        "version": 1, "id": id, "type": kind,
        "timestamp": "2026-01-02T03:04:05Z", "harness": "pi",
        "project_id": "proj", "session_id": session, "correlation_id": "c1",
        "payload": payload
    })
}

fn input(id: &str, session: &str, text: &str) -> Value {
    event(id, session, "input.received", json!({ "text": text }))
}

async fn client(ep: &Endpoint) -> Client {
    Client::connect(ep, HelloOptions::new("pipeline-e2e"))
        .await
        .unwrap()
}

/// Memory writes are asynchronous (single writer): wait until `n` live memories exist.
async fn wait_for_memories(db: &std::path::Path, n: i64) {
    let admin = MemoryAdmin::new(db);
    let start = Instant::now();
    while admin.stats(None).map(|s| s.live).unwrap_or(0) < n {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "memory not persisted"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn sources(d: &Decision) -> Vec<(String, String)> {
    match d {
        Decision::Enrich { derived } => derived
            .iter()
            .map(|d| (d.source.clone(), d.provenance.clone()))
            .collect(),
        other => panic!("expected enrich, got {other:?}"),
    }
}

#[tokio::test]
async fn forge_then_memory_in_order_and_original_untouched() {
    let r = start();
    let mut c = client(&r.ep).await;

    let v = c
        .send_event_value(input(
            "d1",
            "s1",
            "Remember that the login flow uses OAuth device codes",
        ))
        .await
        .unwrap();
    assert!(v.degraded.is_empty(), "{:?}", v.degraded);
    wait_for_memories(&r.db, 1).await;

    let text = "fix the bug in the login flow please";
    let v = c.send_event_value(input("d2", "s2", text)).await.unwrap();
    assert_eq!(
        sources(&v.decision),
        vec![
            ("forge".to_string(), "inferred".to_string()),
            ("memory".to_string(), "recalled".to_string())
        ],
        "documented order: Forge before Memory"
    );
    if let Decision::Enrich { derived } = &v.decision {
        assert!(derived[1].text.contains("OAuth device codes"));
        // Derived text is a separate block: it never replaces or embeds a rewrite of the prompt.
        assert!(derived.iter().all(|d| d.provenance != "user"));
        assert!(
            !derived[0].text.contains(text) || derived[0].text.contains("NOT user instructions")
        );
    }

    // Specific request with no related memory: nothing is added (pure passthrough).
    let v = c
        .send_event_value(input("d3", "s2", "Fix the failing test in parser.rs"))
        .await
        .unwrap();
    assert_eq!(v.decision, Decision::passthrough("no_enrichment"));

    drop(c);
    r.stop.send(()).unwrap();
    assert_eq!(r.task.await.unwrap(), ServeExit::Shutdown);
}

#[tokio::test]
async fn settlement_consolidates_candidates_and_disable_stops_recall() {
    let r = start();
    let mut c = client(&r.ep).await;

    c.send_event_value(event(
        "c1",
        "s1",
        "memory.candidate",
        json!({"text": "the benchmark suite lives under bench/ and needs nightly rust", "category": "repo"}),
    ))
    .await
    .unwrap();
    // Unverified: not stored before settlement.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(MemoryAdmin::new(r.db.clone()).stats(None).unwrap().total, 0);
    c.send_event_value(event(
        "c2",
        "s1",
        "agent.settled",
        json!({"outcome": "ok", "duration_ms": 10}),
    ))
    .await
    .unwrap();
    wait_for_memories(&r.db, 1).await;

    let ask = "how do I run the benchmark suite with nightly rust";
    let v = c.send_event_value(input("c3", "s9", ask)).await.unwrap();
    assert!(matches!(v.decision, Decision::Enrich { .. }));

    // The user switches memory off; the daemon honours it within its settings cache TTL.
    let admin = MemoryAdmin::new(r.db.clone());
    admin.set_enabled(None, false).unwrap();
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let v = c.send_event_value(input("c4", "s9", ask)).await.unwrap();
    assert!(v.decision.is_passthrough(), "memory disabled: {v:?}");

    // Purge removes data, provenance included.
    admin.set_enabled(None, true).unwrap();
    let n = admin
        .purge(kepler_storage::PurgeFilter {
            all: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(n, 1);
    assert!(admin
        .list(&MemoryFilter {
            limit: 10,
            include_superseded: true,
            ..Default::default()
        })
        .unwrap()
        .is_empty());

    drop(c);
    r.stop.send(()).unwrap();
    r.task.await.unwrap();
}

#[tokio::test]
async fn secrets_never_reach_the_memory_database() {
    let r = start();
    let mut c = client(&r.ep).await;
    c.send_event_value(input(
        "s1",
        "s1",
        "remember that we deploy using api_key=sk-abcdefghijklmnopqrstuvwxyz0123456789 on fridays",
    ))
    .await
    .unwrap();
    wait_for_memories(&r.db, 1).await;
    drop(c);
    r.stop.send(()).unwrap();
    r.task.await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut all = std::fs::read(&r.db).unwrap();
    all.extend(std::fs::read(r.db.with_extension("db-wal")).unwrap_or_default());
    assert!(!String::from_utf8_lossy(&all).contains("sk-abcdefghijklmnop"));
    assert!(String::from_utf8_lossy(&all).contains("fridays"));
}

#[tokio::test]
async fn guardian_preflight_rules_and_result_scan_through_the_daemon() {
    let r = start_with(|cfg| {
        cfg.pipeline.guardian_rules = kepler_policy::GuardianRules::parse(
            "deny-path infra/prod\napprove-command kubectl apply\n",
        )
        .0;
    });
    let mut c = client(&r.ep).await;
    let call = |id: &str, payload: Value| event(id, "g1", "tool.call.requested", payload);

    let v = c
        .send_event_value(call("t1", json!({"tool":"bash","command":"cat .env"})))
        .await
        .unwrap();
    assert!(matches!(v.decision, Decision::Deny { .. }));
    let v = c
        .send_event_value(call(
            "t2",
            json!({"tool":"write","paths":["infra/prod/main.tf"]}),
        ))
        .await
        .unwrap();
    assert_eq!(
        v.decision,
        Decision::Deny {
            reason: "user_rule_deny_path".into()
        }
    );
    let v = c
        .send_event_value(call(
            "t3",
            json!({"tool":"bash","command":"kubectl apply -f x.yaml"}),
        ))
        .await
        .unwrap();
    assert!(matches!(v.decision, Decision::RequireApproval { .. }));
    let v = c
        .send_event_value(call("t4", json!({"tool":"bash","command":"cargo test"})))
        .await
        .unwrap();
    assert!(v.decision.is_passthrough());

    // A tool result leaking a secret: redact verdict, secret neither echoed nor persisted.
    let secret = "AKIAIOSFODNN7EXAMPLE";
    let v = c
        .send_event_value(event(
            "t5",
            "g1",
            "tool.call.completed",
            json!({"tool":"bash","ok":true,"output_bytes":50,"duration_ms":4,
                   "output_excerpt": format!("config dump: aws_key {secret} done")}),
        ))
        .await
        .unwrap();
    match &v.decision {
        Decision::Redact { kinds, .. } => assert!(kinds.contains(&"aws_access_key".to_string())),
        d => panic!("{d:?}"),
    }
    assert!(!serde_json::to_string(&v).unwrap().contains(secret));
    drop(c);
    r.stop.send(()).unwrap();
    r.task.await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut all = std::fs::read(&r.db).unwrap();
    all.extend(std::fs::read(r.db.with_extension("db-wal")).unwrap_or_default());
    assert!(!String::from_utf8_lossy(&all).contains(secret));
    let reader = kepler_storage::Reader::open(&r.db).unwrap();
    let decisions: Vec<String> = reader
        .recent_events(10)
        .unwrap()
        .into_iter()
        .map(|e| e.decision)
        .collect();
    assert!(decisions.contains(&"redact".to_string()), "{decisions:?}");
}

#[tokio::test]
async fn token_metrics_are_labeled_estimated_or_measured() {
    let r = start();
    let mut c = client(&r.ep).await;
    // Kepler's own derived context: an estimate.
    let v = c
        .send_event_value(input("m1", "m1", "fix the bug in the login flow please"))
        .await
        .unwrap();
    assert!(v.estimated_tokens.is_some_and(|t| t > 0), "{v:?}");
    // Harness-reported usage keeps its own label.
    c.send_event_value(event(
        "m2",
        "m1",
        "context.prepared",
        json!({"items": 4, "tokens": 1234, "estimated": false}),
    ))
    .await
    .unwrap();
    let start = Instant::now();
    let reader = loop {
        let rd = kepler_storage::Reader::open(&r.db).unwrap();
        if rd.recent_metrics(10).unwrap().len() >= 2 {
            break rd;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "metrics not written"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let m = reader.recent_metrics(10).unwrap();
    let derived = m.iter().find(|m| m.kind == "derived_context").unwrap();
    assert!(derived.estimated);
    let harness = m.iter().find(|m| m.kind == "harness_context").unwrap();
    assert!(!harness.estimated);
    assert_eq!(harness.tokens, 1234);
    drop(c);
    r.stop.send(()).unwrap();
    r.task.await.unwrap();
}

#[tokio::test]
async fn focus_advisories_travel_with_the_verdict_and_can_be_disabled() {
    for (focus, expect) in [(true, true), (false, false)] {
        let r = start_with(|c| c.pipeline.focus = focus);
        let mut c = client(&r.ep).await;
        let mut codes: Vec<String> = Vec::new();
        for i in 0..5 {
            let ev = event(
                &format!("t{i}"),
                "sf",
                "tool.call.requested",
                json!({"tool": "bash", "command": "cargo test"}),
            );
            let v = c.send_event_value(ev).await.unwrap();
            assert!(v.decision.is_passthrough(), "{:?}", v.decision);
            codes.extend(v.advisories.into_iter().map(|a| a.code));
        }
        let found = codes.iter().any(|c| c == "loop.repeated_call");
        assert_eq!(found, expect, "{codes:?}");
        let _ = r.stop.send(());
        let _ = r.task.await;
    }
}

#[tokio::test]
async fn sentinel_persists_path_findings_and_advises_on_serious_ones() {
    let r = start();
    let mut c = client(&r.ep).await;
    let v = c
        .send_event_value(event(
            "f1",
            "ss",
            "files.changed",
            json!({"paths": [".env", "src/lib.rs", ".github/workflows/ci.yml"]}),
        ))
        .await
        .unwrap();
    assert!(v.decision.is_passthrough());
    assert!(v
        .advisories
        .iter()
        .any(|a| a.code == "audit.high_severity_path"));
    let reader = kepler_storage::Reader::open(&r.db).unwrap();
    let start = Instant::now();
    let rows = loop {
        let rows = reader
            .findings(&kepler_storage::FindingFilter {
                limit: 20,
                ..Default::default()
            })
            .unwrap();
        if rows.len() >= 2 || start.elapsed() > Duration::from_secs(5) {
            break rows;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let rules: Vec<&str> = rows.iter().map(|r| r.rule_id.as_str()).collect();
    assert!(
        rules.contains(&"path.sensitive") && rules.contains(&"path.ci_workflow"),
        "{rules:?}"
    );
    assert!(rows
        .iter()
        .all(|r| r.source == "sentinel" && r.project_id == "proj"));
    let _ = r.stop.send(());
    let _ = r.task.await;
}
