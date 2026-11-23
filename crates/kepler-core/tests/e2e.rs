#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kepler_core::{
    daemon::{Daemon, DaemonConfig},
    doctor,
    emit::emit_in_process,
    emit_one_shot, EmitOptions, EmitOutcome,
};
use kepler_events::Envelope;
use kepler_ipc::{Client, ClientError, Endpoint, HelloOptions, ServeExit, ServerFrame};
use kepler_policy::Decision;
use kepler_storage::Reader;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::task::JoinHandle;

static N: AtomicU64 = AtomicU64::new(0);

fn unique() -> u64 {
    N.fetch_add(1, Ordering::SeqCst)
}

fn endpoint() -> Endpoint {
    Endpoint::new(&format!("e2e{}-{}", std::process::id(), unique())).unwrap()
}

fn tmp_db() -> PathBuf {
    let d = std::env::temp_dir().join(format!("kepler-e2e-{}-{}", std::process::id(), unique()));
    std::fs::create_dir_all(&d).unwrap();
    d.join("kepler.db")
}

struct Running {
    ep: Endpoint,
    task: JoinHandle<ServeExit>,
    stop: tokio::sync::oneshot::Sender<()>,
    db: Option<PathBuf>,
}

fn start(with_db: bool, idle: Option<Duration>) -> Running {
    let ep = endpoint();
    let db = with_db.then(tmp_db);
    let mut cfg = DaemonConfig::new(ep.clone());
    cfg.db_path = db.clone();
    cfg.idle_shutdown = idle;
    let daemon = Daemon::start(cfg).unwrap();
    assert!(daemon.warnings.is_empty(), "{:?}", daemon.warnings);
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(daemon.serve(async move {
        let _ = rx.await;
    }));
    Running { ep, task, stop, db }
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
    Client::connect(ep, HelloOptions::new("e2e")).await.unwrap()
}

#[tokio::test]
async fn client_to_daemon_to_passthrough_and_metadata_persisted() {
    let r = start(true, None);
    let mut c = client(&r.ep).await;

    let v = c
        .send_event_value(input("e1", "s1", "Fix the failing test in parser.rs"))
        .await
        .unwrap();
    assert_eq!(v.decision, Decision::passthrough("no_enrichment"));
    let v = c.send_event_value(input("e2", "s1", "ok")).await.unwrap();
    assert!(v.decision.is_passthrough());
    let v = c
        .send_event_value(event(
            "e3",
            "s1",
            "tool.call.requested",
            json!({"tool":"bash","command":"cargo test"}),
        ))
        .await
        .unwrap();
    assert!(v.decision.is_passthrough());
    let v = c
        .send_event_value(event(
            "e4",
            "s1",
            "tool.call.requested",
            json!({"tool":"bash","command":"cat .env"}),
        ))
        .await
        .unwrap();
    assert!(matches!(v.decision, Decision::Deny { .. }));
    let v = c
        .send_event_value(event(
            "e5",
            "s1",
            "tool.call.requested",
            json!({"tool":"bash","command":"rm -rf /"}),
        ))
        .await
        .unwrap();
    assert!(matches!(v.decision, Decision::RequireApproval { .. }));
    drop(c);

    r.stop.send(()).unwrap();
    assert_eq!(r.task.await.unwrap(), ServeExit::Shutdown);
    // Writer is flushed when the daemon (and its Core) are dropped.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let reader = Reader::open(r.db.as_ref().unwrap()).unwrap();
    assert_eq!(reader.event_count().unwrap(), 5);
    let evs = reader.recent_events(10).unwrap();
    assert_eq!(evs[0].decision, "require_approval");
    assert_eq!(evs[1].decision, "deny");
    // Raw prompt/command text is never stored.
    let bytes = std::fs::read(r.db.as_ref().unwrap()).unwrap();
    let wal = std::fs::read(r.db.as_ref().unwrap().with_extension("db-wal")).unwrap_or_default();
    for hay in [&bytes, &wal] {
        assert!(!hay
            .windows(10)
            .any(|w| w == b"parser.rs\x00" || w == b"cat .env\x00"));
        assert!(!String::from_utf8_lossy(hay).contains("Fix the failing test"));
    }
}

#[tokio::test]
async fn forged_and_malformed_session_ids_are_rejected() {
    let r = start(false, None);
    let mut c = client(&r.ep).await;
    for bad in [
        "../../etc/passwd",
        "a b",
        "",
        "x/y",
        "..\\..",
        "s\u{0}1",
        &"s".repeat(200),
        "ünï",
    ] {
        let err = c
            .send_event_value(input("e1", bad, "hello there"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ClientError::Rejected { ref code, fatal: false, .. } if code == "invalid_field" || code == "field_too_large"),
            "{bad:?}: {err}"
        );
        // a success in between resets the consecutive-error limit
        c.ping().await.unwrap();
    }
    // connection still healthy afterwards
    c.send_event_value(input("e2", "good", "hello there"))
        .await
        .unwrap();
}

#[tokio::test]
async fn live_session_cannot_be_impersonated_by_another_connection() {
    let r = start(false, None);
    let mut owner = client(&r.ep).await;
    let mut attacker = client(&r.ep).await;
    owner
        .send_event_value(input("e1", "victim", "hello there"))
        .await
        .unwrap();
    let err = attacker
        .send_event_value(input("e2", "victim", "evil"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ClientError::Rejected { ref code, .. } if code == "session_conflict"),
        "{err}"
    );
    // After the owner closes, the id is free again.
    drop(owner);
    tokio::time::sleep(Duration::from_millis(200)).await;
    attacker
        .send_event_value(input("e3", "victim", "legit now"))
        .await
        .unwrap();
}

#[tokio::test]
async fn session_closed_releases_the_binding_and_per_connection_limit_holds() {
    let r = start(false, None);
    let mut a = client(&r.ep).await;
    let mut b = client(&r.ep).await;
    a.send_event_value(input("e1", "s-a", "hello there"))
        .await
        .unwrap();
    a.send_event_value(event("e2", "s-a", "session.closed", json!({})))
        .await
        .unwrap();
    b.send_event_value(input("e3", "s-a", "hello there"))
        .await
        .unwrap();
    for i in 0..16 {
        a.send_event_value(input(&format!("x{i}"), &format!("many-{i}"), "hello there"))
            .await
            .unwrap();
    }
    let err = a
        .send_event_value(input("x99", "many-99", "hello there"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ClientError::Rejected { ref code, .. } if code == "session_limit"),
        "{err}"
    );
}

#[tokio::test]
async fn malformed_events_never_break_the_daemon() {
    let r = start(false, None);
    let mut c = client(&r.ep).await;
    let junk: Vec<Value> = vec![
        json!(null),
        json!(42),
        json!("string"),
        json!([]),
        json!({}),
        json!({"version": 2}),
        json!({"version": 1, "type": "nope"}),
        event("e", "s", "input.received", json!({"text": 5})),
        event(
            "e",
            "s",
            "input.received",
            json!({"text": "x".repeat(70_000)}),
        ),
        event("e", "s", "tool.call.requested", json!({"tool": ""})),
    ];
    for j in junk {
        let err = c.send_event_value(j).await.unwrap_err();
        assert!(matches!(err, ClientError::Rejected { .. }), "{err}");
        // keep the consecutive-error counter from closing the connection
        c.ping().await.unwrap();
    }
    // raw garbage on the wire is also survivable on a fresh connection
    let mut raw = client(&r.ep).await;
    raw.send_raw(b"\xff\xfe not utf8 \n").await.unwrap();
    assert!(matches!(
        raw.recv().await.unwrap(),
        ServerFrame::Error { .. }
    ));
    client(&r.ep).await.ping().await.unwrap();
}

#[tokio::test]
async fn event_flood_from_many_connections_is_fully_answered() {
    let r = start(true, None);
    let mut tasks = Vec::new();
    for c in 0..16 {
        let ep = r.ep.clone();
        tasks.push(tokio::spawn(async move {
            let mut cl = client(&ep).await;
            let mut ok = 0;
            for i in 0..250 {
                let v = cl
                    .send_event_value(input(
                        &format!("c{c}-{i}"),
                        &format!("flood-{c}"),
                        "refactor the parser please",
                    ))
                    .await
                    .unwrap();
                assert!(v.decision.is_passthrough());
                ok += 1;
            }
            ok
        }));
    }
    let mut total = 0;
    for t in tasks {
        total += t.await.unwrap();
    }
    assert_eq!(total, 4000);
    // persistence is best effort under flood: rows <= events, daemon stayed responsive
    client(&r.ep).await.ping().await.unwrap();
}

#[tokio::test]
async fn missing_core_means_passthrough_and_invalid_events_never_spawn() {
    let ep = endpoint();
    let spawned = AtomicUsize::new(0);
    let spawn = || {
        spawned.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("no daemon in this test"))
    };
    let valid = input("e1", "s1", "Fix the failing test in parser.rs");

    // no autostart configured
    let out = emit_one_shot(&ep, valid.clone(), &EmitOptions::default()).await;
    assert_eq!(
        out,
        EmitOutcome::Passthrough {
            reason: "core_unavailable".into()
        }
    );

    // autostart attempted once, fails => still passthrough
    let opts = EmitOptions {
        spawn_daemon: Some(&spawn),
        ..EmitOptions::default()
    };
    let out = emit_one_shot(&ep, valid, &opts).await;
    assert_eq!(
        out,
        EmitOutcome::Passthrough {
            reason: "core_spawn_failed".into()
        }
    );
    assert_eq!(spawned.load(Ordering::SeqCst), 1);

    // invalid event: rejected locally, no spawn
    let out = emit_one_shot(&ep, json!({"version": 9}), &opts).await;
    assert!(matches!(out, EmitOutcome::Rejected { ref code, .. } if code == "unsupported_version"));
    assert_eq!(spawned.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lazy_start_on_first_event_then_idle_shutdown() {
    let ep = endpoint();
    let started = std::sync::Arc::new(AtomicUsize::new(0));
    let handle: std::sync::Arc<std::sync::Mutex<Option<JoinHandle<ServeExit>>>> =
        Default::default();
    {
        let (ep2, started2, handle2) = (ep.clone(), started.clone(), handle.clone());
        let spawn = move || {
            started2.fetch_add(1, Ordering::SeqCst);
            let mut cfg = DaemonConfig::new(ep2.clone());
            cfg.idle_shutdown = Some(Duration::from_millis(500));
            let d = Daemon::start(cfg).map_err(std::io::Error::other)?;
            *handle2.lock().unwrap() = Some(tokio::spawn(d.serve(std::future::pending())));
            Ok(())
        };
        let opts = EmitOptions {
            spawn_daemon: Some(&spawn),
            ..EmitOptions::default()
        };
        let out = emit_one_shot(
            &ep,
            input("e1", "s1", "Fix the failing test in parser.rs"),
            &opts,
        )
        .await;
        assert!(matches!(out, EmitOutcome::Verdict(_)), "{out:?}");
        // second event reuses the running daemon
        let out = emit_one_shot(&ep, input("e2", "s2", "another real instruction"), &opts).await;
        assert!(matches!(out, EmitOutcome::Verdict(_)));
    }
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "daemon must start lazily exactly once"
    );
    let task = handle.lock().unwrap().take().unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, ServeExit::IdleTimeout);
    // endpoint is free again after idle shutdown
    let mut cfg = DaemonConfig::new(ep);
    cfg.idle_shutdown = None;
    assert!(Daemon::start(cfg).is_ok());
}

#[tokio::test]
async fn second_daemon_for_same_user_is_refused() {
    let r = start(false, None);
    let err = Daemon::start(DaemonConfig::new(r.ep.clone()))
        .err()
        .unwrap();
    assert!(matches!(err, kepler_core::DaemonError::AlreadyRunning));
}

#[tokio::test]
async fn storage_failure_does_not_block_the_daemon() {
    // Parent "directory" is actually a file: the database cannot be created.
    let file = tmp_db().with_file_name("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let ep = endpoint();
    let mut cfg = DaemonConfig::new(ep.clone());
    cfg.db_path = Some(file.join("kepler.db"));
    let daemon = Daemon::start(cfg).unwrap();
    assert_eq!(daemon.warnings.len(), 1, "{:?}", daemon.warnings);
    assert!(daemon.warnings[0].contains("persistence disabled"));
    let _task = tokio::spawn(daemon.serve(std::future::pending()));
    let mut c = client(&ep).await;
    let v = c
        .send_event_value(input("e1", "s1", "refactor the parser please"))
        .await
        .unwrap();
    assert!(v.decision.is_passthrough());
}

#[tokio::test]
async fn doctor_reports_daemon_database_and_capabilities() {
    let r = start(true, None);
    // make sure the db exists
    let mut c = client(&r.ep).await;
    c.send_event_value(input("e1", "s1", "refactor the parser please"))
        .await
        .unwrap();
    let db = r.db.clone().unwrap();
    let report = doctor::run(&r.ep, Ok(db.as_path())).await;
    let j = report.to_json();
    assert_eq!(j["ipc"]["daemon"]["running"], true);
    assert_eq!(j["database"]["exists"], true);
    assert_eq!(j["database"]["fts5"], true);
    assert_eq!(j["database"]["journal_mode"], "wal");
    assert_eq!(j["capabilities"]["contract"].as_array().unwrap().len(), 8);
    assert_eq!(j["runtime"]["llm_or_network_calls"], false);
    assert!(report.to_text().contains("Capabilities"));

    // no daemon, no db: still a report, never creates anything
    let ep2 = endpoint();
    let missing = tmp_db();
    let report = doctor::run(&ep2, Ok(missing.as_path())).await;
    let j = report.to_json();
    assert_eq!(j["ipc"]["daemon"]["running"], false);
    assert_eq!(j["database"]["exists"], false);
    assert!(!missing.exists());
}

#[test]
fn in_process_evaluation_matches_daemon_policy() {
    let out = emit_in_process(input("e1", "s", "ok"));
    assert!(matches!(out, EmitOutcome::Verdict(ref v) if v.decision.is_passthrough()));
    let out = emit_in_process(event(
        "e",
        "s",
        "tool.call.requested",
        json!({"tool":"bash","paths":[".env"]}),
    ));
    assert!(
        matches!(out, EmitOutcome::Verdict(ref v) if matches!(v.decision, Decision::Deny { .. }))
    );
    assert!(matches!(
        emit_in_process(json!({})),
        EmitOutcome::Rejected { .. }
    ));
    // an Envelope parsed from the same JSON agrees
    assert!(Envelope::from_value(input("e1", "s", "ok")).is_ok());
}
