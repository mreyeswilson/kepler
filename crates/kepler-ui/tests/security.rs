#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Security and functional tests of the Genesis UI server over real TCP connections.

use kepler_memory::MemoryAdmin;
use kepler_storage::{
    create_run, store_findings, Confidence, NewFinding, NewRun, NewTask, Scope, Severity, Writer,
};
use kepler_ui::{start, UiConfig, UiServer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

static N: AtomicU64 = AtomicU64::new(0);

struct Reply {
    status: u16,
    headers: String,
    body: String,
}

async fn raw(port: u16, request: &str) -> Reply {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    Reply {
        status,
        headers: head.to_string(),
        body: body.to_string(),
    }
}

struct Fixture {
    server: UiServer,
    port: u16,
}

impl Fixture {
    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    async fn get(&self, path: &str, extra: &str) -> Reply {
        raw(
            self.port,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: {}\r\n{extra}\r\n",
                self.host()
            ),
        )
        .await
    }

    async fn authed(&self, path: &str) -> Reply {
        let t = &self.server.token;
        self.get(path, &format!("Cookie: kepler_ui={t}\r\n")).await
    }
}

async fn fixture(allow_actions: bool) -> Fixture {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("kepler-ui-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("k.db");
    {
        let w = Writer::start(&db, Default::default()).unwrap();
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
                    path: "src/<img src=x onerror=alert(1)>.rs".into(),
                    line: Some(3),
                    message: "unsafe block".into(),
                    evidence: "unsafe {".into(),
                }],
            )
        })
        .unwrap();
        w.exec(Duration::from_secs(5), |c| {
            create_run(
                c,
                &NewRun {
                    run_id: "run1".into(),
                    project_id: "p".into(),
                    repo_path: "C:/x".into(),
                    base_ref: "abc".into(),
                    name: "demo".into(),
                    max_parallel: 2,
                },
                &[NewTask {
                    key: "danger".into(),
                    description: "d".into(),
                    argv: vec!["git".into(), "push".into(), "--force".into()],
                    timeout_secs: 10,
                    max_retries: 0,
                    needs_approval: true,
                    approval_reason: "destructive_command".into(),
                    deps: vec![],
                }],
            )
        })
        .unwrap();
        drop(w);
        MemoryAdmin::new(&db)
            .add(
                "p",
                Scope::Project,
                "",
                "<script>alert('xss')</script> use pnpm",
            )
            .unwrap();
    }
    let server = start(UiConfig {
        db_path: db,
        port: 0,
        allow_actions,
    })
    .await
    .unwrap();
    let port = server.addr.port();
    Fixture { server, port }
}

#[tokio::test]
async fn no_token_is_401_and_nothing_leaks() {
    let f = fixture(false).await;
    for path in [
        "/",
        "/api/overview",
        "/api/memories",
        "/api/findings",
        "/api/orbit",
        "/app.js",
        "/nope",
    ] {
        let r = f.get(path, "").await;
        assert_eq!(r.status, 401, "{path}");
        assert!(
            !r.body.contains("pnpm") && !r.body.contains("rust.unsafe"),
            "{path}"
        );
    }
    // wrong tokens: cookie, bearer, query
    let t = &f.server.token;
    let wrong = format!(
        "{}{}",
        &t[..t.len() - 1],
        if t.ends_with('0') { '1' } else { '0' }
    );
    assert_eq!(
        f.get("/api/overview", &format!("Cookie: kepler_ui={wrong}\r\n"))
            .await
            .status,
        401
    );
    assert_eq!(
        f.get(
            "/api/overview",
            &format!("Authorization: Bearer {wrong}\r\n")
        )
        .await
        .status,
        401
    );
    assert_eq!(
        f.get("/api/overview", "Cookie: kepler_ui=\r\n")
            .await
            .status,
        401
    );
    assert_eq!(
        f.get(&format!("/api/overview?t={t}"), "").await.status,
        401,
        "the query token is only for the landing page"
    );
    assert_eq!(f.get(&format!("/?t={wrong}"), "").await.status, 401);
}

#[tokio::test]
async fn landing_with_token_sets_an_httponly_strict_cookie_then_works() {
    let f = fixture(false).await;
    let r = f.get(&format!("/?t={}", f.server.token), "").await;
    assert_eq!(r.status, 302);
    assert!(r.headers.contains("Location: /"));
    let set = r
        .headers
        .lines()
        .find(|l| l.starts_with("Set-Cookie"))
        .unwrap();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Strict"),
        "{set}"
    );
    assert!(!r
        .headers
        .lines()
        .any(|l| l.starts_with("Location") && l.contains(&f.server.token)));
    let page = f.authed("/").await;
    assert_eq!(page.status, 200);
    assert!(page.body.contains("<title>Kepler</title>"));
    for h in [
        "Content-Security-Policy",
        "X-Content-Type-Options: nosniff",
        "Referrer-Policy: no-referrer",
        "Cache-Control: no-store",
        "X-Frame-Options: DENY",
    ] {
        assert!(page.headers.contains(h), "missing {h}");
    }
    assert!(page.headers.contains("script-src 'self'") && !page.headers.contains("unsafe-inline"));
    assert!(
        !page.body.contains("<script>") || page.body.contains("<script src="),
        "no inline script"
    );
    // bearer works too (scripting / tests)
    let t = &f.server.token;
    assert_eq!(
        f.get("/api/overview", &format!("Authorization: Bearer {t}\r\n"))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn dns_rebinding_and_foreign_hosts_are_rejected_even_with_a_valid_token() {
    let f = fixture(false).await;
    let t = &f.server.token;
    for host in [
        "evil.example".to_string(),
        format!("evil.example:{}", f.port),
        "127.0.0.1".to_string(), // missing port
        format!("127.0.0.1.evil.example:{}", f.port),
        format!("[::1]:{}", f.port),
        format!("127.0.0.1:{}", f.port + 1),
        String::new(),
    ] {
        let r = raw(
            f.port,
            &format!("GET /api/overview HTTP/1.1\r\nHost: {host}\r\nCookie: kepler_ui={t}\r\n\r\n"),
        )
        .await;
        assert_eq!(r.status, 421, "Host {host:?}");
        assert!(!r.body.contains("version"));
    }
    // no Host header at all (HTTP/1.0 style)
    let r = raw(
        f.port,
        &format!("GET /api/overview HTTP/1.0\r\nCookie: kepler_ui={t}\r\n\r\n"),
    )
    .await;
    assert_eq!(r.status, 421);
    // localhost spelling is allowed
    let r = raw(
        f.port,
        &format!(
            "GET /api/overview HTTP/1.1\r\nHost: localhost:{}\r\nCookie: kepler_ui={t}\r\n\r\n",
            f.port
        ),
    )
    .await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn cross_origin_requests_are_refused() {
    let f = fixture(false).await;
    let t = f.server.token.clone();
    for extra in [
        "Origin: http://evil.example\r\n".to_string(),
        "Origin: null\r\n".to_string(),
        format!("Origin: http://127.0.0.1:{}\r\n", f.port + 1),
        "Sec-Fetch-Site: cross-site\r\n".to_string(),
        "Sec-Fetch-Site: same-site\r\n".to_string(),
    ] {
        let r = f
            .get(
                "/api/overview",
                &format!("Cookie: kepler_ui={t}\r\n{extra}"),
            )
            .await;
        assert_eq!(r.status, 403, "{extra:?}");
    }
    let ok = f
        .get(
            "/api/overview",
            &format!("Cookie: kepler_ui={t}\r\nOrigin: http://127.0.0.1:{}\r\nSec-Fetch-Site: same-origin\r\n", f.port),
        )
        .await;
    assert_eq!(ok.status, 200);
}

#[tokio::test]
async fn api_returns_data_as_json_and_never_as_markup() {
    let f = fixture(false).await;
    let o = f.authed("/api/overview").await;
    assert_eq!(o.status, 200);
    let v: serde_json::Value = serde_json::from_str(&o.body).unwrap();
    assert_eq!(v["actions_enabled"], false);
    assert_eq!(v["memory"]["live"], 1);
    assert_eq!(v["findings"]["open"], 1);
    assert_eq!(v["orbit"]["tasks_awaiting_approval"], 1);
    let m = f.authed("/api/memories").await;
    assert!(m.headers.contains("application/json") && m.headers.contains("nosniff"));
    assert!(
        m.body.contains("<script>"),
        "stored text is returned verbatim as data"
    );
    let fi: serde_json::Value =
        serde_json::from_str(&f.authed("/api/findings?min_severity=medium").await.body).unwrap();
    assert_eq!(fi["findings"][0]["rule"], "rust.unsafe");
    assert_eq!(
        f.authed("/api/findings?min_severity=bogus").await.status,
        500
    );
    let orbit: serde_json::Value =
        serde_json::from_str(&f.authed("/api/orbit").await.body).unwrap();
    assert_eq!(orbit["runs"][0]["tasks"][0]["status"], "awaiting_approval");
    assert_eq!(f.authed("/api/events").await.status, 200);
    assert_eq!(f.authed("/api/metrics").await.status, 200);
    assert_eq!(f.authed("/api/nothing").await.status, 404);
    // the page script never uses innerHTML / eval / document.write
    let js = f.authed("/app.js").await.body;
    for bad in [
        "innerHTML",
        "outerHTML",
        "eval(",
        "document.write",
        "insertAdjacentHTML",
    ] {
        assert!(!js.contains(bad), "{bad} in app.js");
    }
}

#[tokio::test]
async fn actions_are_off_by_default_and_need_json_origin_and_confirmation() {
    let f = fixture(false).await;
    let t = f.server.token.clone();
    let body = r#"{"run":"run1","task":"danger","grant":true,"confirm":"danger"}"#;
    let post = |port: u16, extra: String, host: String, body: &str| {
        let req = format!(
            "POST /api/orbit/resolve HTTP/1.1\r\nHost: {host}\r\nCookie: kepler_ui={t}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        async move { raw(port, &req).await }
    };
    let json = "Content-Type: application/json\r\n".to_string();
    assert_eq!(
        post(f.port, json.clone(), f.host(), body).await.status,
        403,
        "disabled by default"
    );
    f.server.shutdown();

    let f = fixture(true).await;
    let t2 = f.server.token.clone();
    let post = |extra: String, host: String, body: String, tok: String| {
        let req = format!(
            "POST /api/orbit/resolve HTTP/1.1\r\nHost: {host}\r\nCookie: kepler_ui={tok}\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let port = f.port;
        async move { raw(port, &req).await }
    };
    let good = r#"{"run":"run1","task":"danger","grant":false,"confirm":"danger"}"#.to_string();
    // missing token
    assert_eq!(
        post(json.clone(), f.host(), good.clone(), "x".into())
            .await
            .status,
        401
    );
    // cross-origin POST
    let cross = format!("{json}Origin: http://evil.example\r\n");
    assert_eq!(
        post(cross, f.host(), good.clone(), t2.clone()).await.status,
        403
    );
    // form content type (the CSRF-friendly one) is refused
    let form = "Content-Type: application/x-www-form-urlencoded\r\n".to_string();
    assert_eq!(
        post(form, f.host(), good.clone(), t2.clone()).await.status,
        415
    );
    // wrong confirmation
    let wrong = r#"{"run":"run1","task":"danger","grant":true,"confirm":"yes"}"#.to_string();
    assert_eq!(
        post(json.clone(), f.host(), wrong, t2.clone()).await.status,
        400
    );
    // malformed JSON / missing fields
    assert_eq!(
        post(json.clone(), f.host(), "{".into(), t2.clone())
            .await
            .status,
        400
    );
    assert_eq!(
        post(json.clone(), f.host(), "{}".into(), t2.clone())
            .await
            .status,
        400
    );
    // oversized body
    let big = format!(r#"{{"x":"{}"}}"#, "a".repeat(8000));
    assert_eq!(
        post(json.clone(), f.host(), big, t2.clone()).await.status,
        431
    );
    // unknown task: nothing waiting
    let unk = r#"{"run":"run1","task":"nope","grant":true,"confirm":"nope"}"#.to_string();
    assert_eq!(
        post(json.clone(), f.host(), unk, t2.clone()).await.status,
        409
    );
    // rebinding host
    assert_eq!(
        post(
            json.clone(),
            "evil.example".into(),
            good.clone(),
            t2.clone()
        )
        .await
        .status,
        421
    );
    // the real thing
    let r = post(json.clone(), f.host(), good, t2.clone()).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let orbit: serde_json::Value =
        serde_json::from_str(&f.authed("/api/orbit").await.body).unwrap();
    assert_eq!(orbit["runs"][0]["tasks"][0]["status"], "denied");
    // a second resolution of the same task is refused
    let again = r#"{"run":"run1","task":"danger","grant":true,"confirm":"danger"}"#.to_string();
    assert_eq!(post(json, f.host(), again, t2).await.status, 409);
}

#[tokio::test]
async fn hostile_requests_are_bounded() {
    let f = fixture(false).await;
    // huge header block
    let big = format!(
        "GET / HTTP/1.1\r\nHost: {}\r\nX-A: {}\r\n\r\n",
        f.host(),
        "a".repeat(40_000)
    );
    assert_eq!(raw(f.port, &big).await.status, 431);
    // too many headers
    let many: String = (0..100).map(|i| format!("X-{i}: v\r\n")).collect();
    let r = raw(
        f.port,
        &format!("GET / HTTP/1.1\r\nHost: {}\r\n{many}\r\n", f.host()),
    )
    .await;
    assert_eq!(r.status, 431);
    // garbage and chunked bodies
    assert_eq!(raw(f.port, "NOT HTTP\r\n\r\n").await.status, 400);
    let chunked = format!("POST /api/orbit/resolve HTTP/1.1\r\nHost: {}\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", f.host());
    assert_eq!(raw(f.port, &chunked).await.status, 400);
    // other methods
    let r = raw(
        f.port,
        &format!(
            "DELETE /api/overview HTTP/1.1\r\nHost: {}\r\n\r\n",
            f.host()
        ),
    )
    .await;
    assert_eq!(r.status, 405);
    // a stalled client is dropped with 408 and does not block others
    let mut slow = TcpStream::connect(("127.0.0.1", f.port)).await.unwrap();
    slow.write_all(b"GET / HTTP/1.1\r\nHost: ").await.unwrap();
    assert_eq!(f.authed("/").await.status, 200);
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), slow.read_to_end(&mut buf)).await;
    assert!(
        String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 408"),
        "{}",
        String::from_utf8_lossy(&buf)
    );
}

#[tokio::test]
async fn binds_loopback_only_and_picks_distinct_random_ports() {
    let a = fixture(false).await;
    let b = fixture(false).await;
    assert!(a.server.addr.ip().is_loopback());
    assert_ne!(a.port, b.port);
    assert_ne!(a.server.token, b.server.token);
    assert_eq!(a.server.token.len(), 64);
}
