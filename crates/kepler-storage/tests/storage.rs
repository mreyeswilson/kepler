#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kepler_events::Envelope;
use kepler_policy::{Decision, PolicyEngine, Verdict};
use kepler_storage::{
    inspect, resolve_within, EventRecord, MemoryRecord, PathError, Reader, StorageError, Writer,
    WriterConfig,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static N: AtomicU64 = AtomicU64::new(0);

fn tmp_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "kepler-storage-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn ev(id: &str, session: &str, kind: &str, payload: serde_json::Value) -> Envelope {
    Envelope::from_value(serde_json::json!({
        "version": 1, "id": id, "type": kind,
        "timestamp": "2026-01-02T03:04:05Z", "harness": "pi",
        "project_id": "proj", "session_id": session, "correlation_id": "c1",
        "payload": payload
    }))
    .unwrap()
}

fn record(e: &Envelope) -> EventRecord {
    EventRecord::from_event(e, &PolicyEngine::default().evaluate(e))
}

fn all_db_bytes(db: &std::path::Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal"] {
        let mut p = db.as_os_str().to_owned();
        p.push(suffix);
        if let Ok(b) = std::fs::read(PathBuf::from(p)) {
            bytes.extend(b);
        }
    }
    bytes
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn creates_wal_database_with_fts5_and_reports_status() {
    let dir = tmp_dir();
    let db = dir.join("sub").join("kepler.db");
    assert!(!inspect(&db).exists);
    let w = Writer::start(&db, WriterConfig::default()).unwrap();
    assert!(w.flush(Duration::from_secs(2)));
    drop(w);
    let s = inspect(&db);
    assert!(s.exists);
    assert_eq!(s.schema_version, Some(kepler_storage::LATEST_VERSION));
    assert_eq!(s.journal_mode.as_deref(), Some("wal"));
    assert_eq!(s.fts5, Some(true));
    assert!(s.error.is_none());
}

#[test]
fn persists_metadata_only_never_prompt_text() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    let w = Writer::start(&db, WriterConfig::default()).unwrap();
    let secret_prompt = "please deploy with password hunter2-UNIQUE-PROMPT-MARKER";
    let e = ev(
        "e1",
        "s1",
        "input.received",
        serde_json::json!({ "text": secret_prompt }),
    );
    assert!(w.record_event(record(&e)));
    let tool = ev(
        "e2",
        "s1",
        "tool.call.requested",
        serde_json::json!({ "tool": "bash", "command": "curl -H 'Authorization: Bearer UNIQUE-CMD-MARKER' x" }),
    );
    assert!(w.record_event(record(&tool)));
    assert!(w.flush(Duration::from_secs(2)));
    drop(w);

    let bytes = all_db_bytes(&db);
    assert!(!contains(&bytes, "UNIQUE-PROMPT-MARKER"), "prompt leaked");
    assert!(!contains(&bytes, "UNIQUE-CMD-MARKER"), "command leaked");

    let r = Reader::open(&db).unwrap();
    assert_eq!(r.event_count().unwrap(), 2);
    let events = r.recent_events(10).unwrap();
    assert_eq!(events[1].kind, "input.received");
    assert_eq!(events[1].decision, "passthrough");
    assert_eq!(events[1].payload_bytes, secret_prompt.len() as i64);
    assert_eq!(r.session_event_count("s1").unwrap(), Some(2));
}

#[test]
fn env_file_content_in_tool_output_never_reaches_disk() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    let w = Writer::start(&db, WriterConfig::default()).unwrap();
    let leak = ev(
        "e1",
        "s1",
        "tool.call.completed",
        serde_json::json!({
            "tool": "bash", "ok": true, "output_bytes": 60, "duration_ms": 3,
            "output_excerpt": "DB_PASSWORD=ENVLEAK-hunter2\nAPI_KEY=ENVLEAK-abc123\n"
        }),
    );
    // The same payload through the policy engine is flagged by Guardian (Delivery 2)...
    let v = PolicyEngine::default().evaluate(&leak);
    assert!(matches!(v.decision, Decision::Redact { .. }), "{v:?}");
    // ...and persistence keeps only metadata.
    assert!(w.record_event(EventRecord::from_event(&leak, &v)));
    // A tool call trying to read .env is denied and its reason is stored, not the command.
    let read_env = ev(
        "e2",
        "s1",
        "tool.call.requested",
        serde_json::json!({ "tool": "read", "paths": [".env"], "command": "cat .env" }),
    );
    let v = PolicyEngine::default().evaluate(&read_env);
    assert!(matches!(v.decision, Decision::Deny { .. }));
    assert!(w.record_event(EventRecord::from_event(&read_env, &v)));
    assert!(w.flush(Duration::from_secs(2)));
    drop(w);
    assert!(!contains(&all_db_bytes(&db), "ENVLEAK"));
}

#[test]
fn memories_are_redacted_and_sensitive_sources_refused() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    let w = Writer::start(&db, WriterConfig::default()).unwrap();
    let t = Duration::from_secs(2);
    let id = w
        .add_memory(
            MemoryRecord {
                project_id: "p1".into(),
                content: "deploy uses cargo; API_KEY=MEMLEAK-xyz".into(),
                source: "user".into(),
                source_path: Some("docs/deploy.md".into()),
            },
            t,
        )
        .unwrap();
    assert!(id > 0);
    w.add_memory(
        MemoryRecord {
            project_id: "p2".into(),
            content: "other project uses cargo too".into(),
            source: "derived".into(),
            source_path: None,
        },
        t,
    )
    .unwrap();

    for (path, content, source) in [
        (Some(".env"), "x", "user"),
        (Some("a/../.env.local"), "x", "user"),
        (Some("~/.ssh/id_rsa"), "x", "user"),
        (None, "   ", "user"),
        (None, "fine", "invented"),
    ] {
        let err = w
            .add_memory(
                MemoryRecord {
                    project_id: "p1".into(),
                    content: content.into(),
                    source: source.into(),
                    source_path: path.map(Into::into),
                },
                t,
            )
            .unwrap_err();
        assert!(matches!(err, StorageError::Rejected(_)), "{path:?} {err}");
    }
    let too_big = MemoryRecord {
        project_id: "p1".into(),
        content: "x".repeat(5000),
        source: "user".into(),
        source_path: None,
    };
    assert!(matches!(
        w.add_memory(too_big, t),
        Err(StorageError::Rejected(_))
    ));
    drop(w);

    assert!(!contains(&all_db_bytes(&db), "MEMLEAK"));
    let r = Reader::open(&db).unwrap();
    // scoped retrieval: only the project's own memory
    let hits = r.search_memories("p1", "cargo", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("[REDACTED]"));
    assert_eq!(r.search_memories("p2", "cargo", 10).unwrap().len(), 1);
    assert!(r.search_memories("p3", "cargo", 10).unwrap().is_empty());
    // FTS syntax in user queries is neutralized
    assert!(r.search_memories("p1", "\" OR 1=1 --", 10).is_ok());
    assert!(r.search_memories("p1", "*", 10).unwrap().is_empty());
}

#[test]
fn reader_never_creates_a_database() {
    let dir = tmp_dir();
    let db = dir.join("missing.db");
    assert!(Reader::open(&db).is_err());
    assert!(!db.exists());
}

#[test]
fn duplicate_events_are_idempotent() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    let w = Writer::start(&db, WriterConfig::default()).unwrap();
    let e = ev("same", "s1", "agent.started", serde_json::json!({}));
    for _ in 0..5 {
        w.record_event(record(&e));
    }
    assert!(w.flush(Duration::from_secs(2)));
    drop(w);
    let r = Reader::open(&db).unwrap();
    assert_eq!(r.event_count().unwrap(), 1);
    assert_eq!(r.session_event_count("s1").unwrap(), Some(1));
}

#[test]
fn full_queue_drops_instead_of_blocking() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    let cfg = WriterConfig {
        queue_capacity: 4,
        ..WriterConfig::default()
    };
    let w = Writer::start(&db, cfg).unwrap();
    let started = std::time::Instant::now();
    let mut accepted = 0u64;
    for i in 0..5000 {
        let e = ev(
            &format!("e{i}"),
            "flood",
            "agent.started",
            serde_json::json!({}),
        );
        if w.record_event(record(&e)) {
            accepted += 1;
        }
    }
    // Never blocks the caller (5000 enqueues attempted well under a few seconds)...
    assert!(started.elapsed() < Duration::from_secs(5));
    // ...and everything is either stored or counted as dropped.
    assert!(w.flush(Duration::from_secs(5)));
    assert_eq!(accepted + w.dropped_events(), 5000);
    drop(w);
    let r = Reader::open(&db).unwrap();
    assert_eq!(r.event_count().unwrap() as u64, accepted);
}

#[test]
fn retention_prunes_oldest_events() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    let cfg = WriterConfig {
        max_events: 100,
        ..WriterConfig::default()
    };
    let w = Writer::start(&db, cfg).unwrap();
    for i in 0..700 {
        let e = ev(
            &format!("e{i}"),
            "s",
            "agent.started",
            serde_json::json!({}),
        );
        while !w.record_event(record(&e)) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    assert!(w.flush(Duration::from_secs(5)));
    drop(w);
    let r = Reader::open(&db).unwrap();
    let n = r.event_count().unwrap();
    assert!(n < 700, "pruning must have happened, have {n}");
    assert!(n >= 100);
    // newest survives
    assert_eq!(r.recent_events(1).unwrap()[0].event_id, "e699");
}

#[test]
fn newer_schema_is_refused_without_modification() {
    let dir = tmp_dir();
    let db = dir.join("kepler.db");
    drop(Writer::start(&db, WriterConfig::default()).unwrap());
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA user_version = 999;").unwrap();
    drop(conn);
    let err = Writer::start(&db, WriterConfig::default()).err().unwrap();
    assert!(
        matches!(err, StorageError::SchemaTooNew { found: 999, .. }),
        "{err}"
    );
}

#[test]
fn path_traversal_is_blocked_by_resolution_helpers() {
    let root = tmp_dir();
    std::fs::create_dir_all(root.join("src")).unwrap();
    assert!(resolve_within(&root, "src/lib.rs").is_ok());
    for bad in [
        "../x",
        "src/../../x",
        "..\\..\\x",
        "src/../../../etc/passwd",
    ] {
        assert_eq!(
            resolve_within(&root, bad),
            Err(PathError::Traversal),
            "{bad}"
        );
    }
    for bad in [
        ".env",
        "src/.env.local",
        "src/../.env",
        "keys/server.pem",
        ".ssh/id_rsa",
    ] {
        assert_eq!(
            resolve_within(&root, bad),
            Err(PathError::Sensitive),
            "{bad}"
        );
    }
}

#[test]
fn verdict_reason_is_redacted_and_bounded() {
    let e = ev("e", "s", "agent.started", serde_json::json!({}));
    let v = Verdict::new(Decision::Deny {
        reason: format!("password=VERDICTLEAK {}", "x".repeat(500)),
    });
    let r = EventRecord::from_event(&e, &v);
    let reason = r.reason.unwrap();
    assert!(!reason.contains("VERDICTLEAK"));
    assert!(reason.len() <= 128);
}
