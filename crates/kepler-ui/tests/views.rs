#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The read-only view endpoints: shape, bounds, auth, and hostile data.

use kepler_storage::{
    store_findings, Confidence, EventRecord, NewFinding, Severity, Writer, WriterConfig,
};
use kepler_ui::{start, UiConfig, UiServer};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

static N: AtomicU64 = AtomicU64::new(0);
const HOSTILE: &str = "<script>alert(1)</script>";

struct Reply {
    status: u16,
    head: String,
    body: String,
}

struct Fx {
    server: UiServer,
    port: u16,
}

async fn raw(port: u16, request: &str) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Reply {
        status: head
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0),
        head: head.to_string(),
        body: body.to_string(),
    }
}

impl Fx {
    async fn get(&self, path: &str, auth: bool, host: &str) -> Reply {
        let c = if auth {
            format!("Cookie: kepler_ui={}\r\n", self.server.token)
        } else {
            String::new()
        };
        raw(
            self.port,
            &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{c}\r\n"),
        )
        .await
    }

    async fn json(&self, path: &str) -> Value {
        let r = self
            .get(path, true, &format!("127.0.0.1:{}", self.port))
            .await;
        assert_eq!(r.status, 200, "{path}: {}", r.body);
        assert!(r.head.contains("application/json"), "{path}");
        assert!(r
            .head
            .to_lowercase()
            .contains("x-content-type-options: nosniff"));
        serde_json::from_str(&r.body).unwrap()
    }
}

fn ev(
    i: usize,
    session: &str,
    kind: &str,
    decision: &'static str,
    reason: Option<&str>,
) -> EventRecord {
    EventRecord {
        event_id: format!("e{i}"),
        kind: kind.into(),
        ts: "2026-01-02T03:04:05Z".into(),
        harness: "claude-code".into(),
        project_id: format!("cc-{HOSTILE}-0123456789"),
        session_id: session.into(),
        correlation_id: "c".into(),
        payload_bytes: 12,
        decision,
        reason: reason.map(String::from),
        actors: Some("forge".into()),
        detail: Some("forge=enrich;rule=vague_task;class=task;conf=decisive;tool=Bash".into()),
        proc_ms: Some(3),
    }
}

async fn fixture() -> Fx {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("kepler-uiv-t-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("k.db");
    {
        let w = Writer::start(&db, WriterConfig::default()).unwrap();
        for i in 0..260 {
            let (kind, d, r) = if i % 5 == 0 {
                ("tool.call.requested", "deny", Some("destructive_command"))
            } else {
                ("input.received", "enrich", None)
            };
            let session = if i % 2 == 0 { HOSTILE } else { "s-normal" };
            while !w.record_event(ev(i, session, kind, d, r)) {
                w.flush(Duration::from_secs(10));
            }
        }
        assert!(w.flush(Duration::from_secs(10)));
        w.exec(Duration::from_secs(5), |c| {
            store_findings(
                c,
                &[NewFinding {
                    project_id: "p".into(),
                    session_id: String::new(),
                    source: "sentinel".into(),
                    rule_id: "rust.unsafe".into(),
                    severity: Severity::High,
                    confidence: Confidence::Suspected,
                    path: format!("src/{HOSTILE}.rs"),
                    line: Some(3),
                    message: "SECRET-MESSAGE".into(),
                    evidence: "SECRET-EVIDENCE".into(),
                }],
            )
        })
        .unwrap();
        drop(w);
    }
    let server = start(UiConfig {
        db_path: db,
        port: 0,
        allow_actions: false,
    })
    .await
    .unwrap();
    let port = server.addr.port();
    Fx { server, port }
}

const VIEWS: [&str; 10] = [
    "/api/summary",
    "/api/modules",
    "/api/sessions",
    "/api/session?id=s-normal",
    "/api/events",
    "/api/forge",
    "/api/guardian",
    "/api/memory",
    "/api/sentinel",
    "/api/vision-focus",
];

#[tokio::test]
async fn every_view_needs_the_token_and_a_loopback_host() {
    let f = fixture().await;
    let host = format!("127.0.0.1:{}", f.port);
    for p in VIEWS {
        assert_eq!(
            f.get(p, false, &host).await.status,
            401,
            "{p} without token"
        );
        assert_eq!(
            f.get(p, true, "evil.example").await.status,
            421,
            "{p} bad host"
        );
        let r = raw(
            f.port,
            &format!(
                "GET {p} HTTP/1.1\r\nHost: {host}\r\nCookie: kepler_ui={}\r\nOrigin: http://evil.example\r\n\r\n",
                f.server.token
            ),
        )
        .await;
        assert_eq!(r.status, 403, "{p} cross-origin");
        let r = raw(
            f.port,
            &format!("POST {p} HTTP/1.1\r\nHost: {host}\r\nCookie: kepler_ui={}\r\nContent-Length: 0\r\n\r\n", f.server.token),
        )
        .await;
        assert!(
            r.status == 404 || r.status == 403,
            "{p} POST -> {}",
            r.status
        );
    }
    f.server.shutdown();
}

#[tokio::test]
async fn views_have_the_documented_shape_and_stay_bounded() {
    let f = fixture().await;
    let s = f.json("/api/summary").await;
    assert_eq!(s["events"]["total"], 260);
    assert!(s["health"]["level"].is_string());
    assert!(s["daemon"]["state"].is_string());
    assert_eq!(
        s["database"]["schema_version"],
        s["database"]["latest_schema"]
    );

    let m = f.json("/api/modules").await;
    let ids: Vec<&str> = m["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["focus", "forge", "memory", "guardian", "orbit", "sentinel", "vision", "genesis"]
    );
    let by = |id: &str| {
        m["modules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by("forge")["state"], "active");
    assert_eq!(by("guardian")["state"], "active");
    assert_eq!(
        by("focus")["state"],
        "none",
        "no advisories, no activity invented"
    );
    assert_eq!(by("genesis")["state"], "not_implemented");
    for x in m["modules"].as_array().unwrap() {
        assert!(!x["limitation"].as_str().unwrap().is_empty());
    }

    // Events: hard cap of 200 rows per page, pagination and filters.
    let e = f.json("/api/events?limit=100000").await;
    assert_eq!(e["events"].as_array().unwrap().len(), 200);
    assert_eq!(e["total"], 260);
    assert_eq!(e["has_more"], true);
    let e2 = f.json("/api/events?limit=200&offset=200").await;
    assert_eq!(e2["events"].as_array().unwrap().len(), 60);
    assert_eq!(e2["has_more"], false);
    let d = f.json("/api/events?decision=deny").await;
    assert_eq!(d["total"], 52);
    let k = f
        .json("/api/events?kind=tool.call.requested&module=forge&window=7d")
        .await;
    assert_eq!(k["total"], 52);
    // Hostile filter values are ignored, never interpolated.
    let h = f.json("/api/events?kind=x'%20OR%201=1--").await;
    assert_eq!(h["total"], 260);
    // Event rows carry metadata only.
    let row = &e["events"][0];
    for key in [
        "kind",
        "decision",
        "reason",
        "payload_bytes",
        "actors",
        "ms",
        "session",
    ] {
        assert!(row.get(key).is_some(), "{key}");
    }
    assert!(row.get("text").is_none() && row.get("prompt").is_none());

    let sess = f.json("/api/sessions?limit=1000").await;
    assert_eq!(sess["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(sess["sessions"][0]["project"].as_str().unwrap().len(), 11);
    let det = f.json("/api/session?id=s-normal&limit=10").await;
    assert_eq!(det["timeline"].as_array().unwrap().len(), 10);
    assert_eq!(det["truncated"], true);
    assert_eq!(
        f.json("/api/session?id=nope").await["error"],
        "unknown session"
    );
    let bad = f
        .get("/api/session", true, &format!("127.0.0.1:{}", f.port))
        .await;
    assert_eq!(bad.status, 500);

    let fo = f.json("/api/forge").await;
    assert_eq!(fo["modes"]["enrich"], 208);
    assert_eq!(fo["classes"]["task"], 208);
    assert_eq!(fo["recent"].as_array().unwrap().len(), 25);
    let g = f.json("/api/guardian").await;
    assert_eq!(g["reasons"][0]["reason"], "destructive_command");
    assert_eq!(g["reasons"][0]["count"], 52);
    assert_eq!(g["top_tools"][0]["tool"], "Bash");
    assert!(g["budgets"]["context_tokens"].as_u64().unwrap() > 0);
    for p in ["/api/memory", "/api/vision-focus"] {
        f.json(p).await;
    }
    f.server.shutdown();
}

#[tokio::test]
async fn hostile_names_travel_as_data_and_sentinel_hides_messages_and_evidence() {
    let f = fixture().await;
    let page = f
        .get("/app.js", true, &format!("127.0.0.1:{}", f.port))
        .await
        .body;
    for bad in [
        "innerHTML",
        "outerHTML",
        "insertAdjacentHTML",
        "document.write",
        "eval(",
        "new Function",
    ] {
        assert!(!page.contains(bad), "{bad} in app.js");
    }
    let sess = f.json("/api/sessions").await.to_string();
    assert!(
        sess.contains(HOSTILE),
        "hostile session id is kept as data (escaped by the page)"
    );
    let s = f
        .get("/api/sentinel", true, &format!("127.0.0.1:{}", f.port))
        .await;
    assert!(s.body.contains("rust.unsafe") && s.body.contains("<script>"));
    assert!(!s.body.contains("SECRET-MESSAGE") && !s.body.contains("SECRET-EVIDENCE"));
    // The session id is also usable as a filter only when it is a plain token.
    let q = f.json("/api/events?session=%3Cscript%3E").await;
    assert_eq!(q["total"], 260, "non-token filter ignored");
    // Responses are never HTML.
    let r = f
        .get("/api/sessions", true, &format!("127.0.0.1:{}", f.port))
        .await;
    assert!(r.head.contains("application/json"));
    assert!(r.head.contains("default-src 'none'"));
    f.server.shutdown();
}
