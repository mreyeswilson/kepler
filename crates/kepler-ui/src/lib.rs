//! Genesis local UI (ADR-0024): a loopback-only, token-protected, read-mostly dashboard.
//!
//! Threat model: other local apps and web pages (including DNS-rebinding and cross-site
//! requests to `127.0.0.1`) must not read Kepler's data or trigger actions.
//!
//! * binds `127.0.0.1` only; the port is random unless given;
//! * a 256-bit random session token is required on every request (cookie set after the first
//!   visit to `/?t=<token>`, or `Authorization: Bearer`), compared in constant time;
//! * the `Host` header must be exactly the loopback origin of this server (rebinding defence),
//!   `Origin`/`Sec-Fetch-Site`, when present, must be same-origin;
//! * every API route is read-only except `POST /api/orbit/resolve`, which exists only when the
//!   server was started with actions enabled, needs JSON, the same origin and an explicit
//!   confirmation field equal to the task key;
//! * strict CSP, no inline script, all dynamic text inserted with `textContent` by the page;
//! * bounded request sizes, one request per connection, read timeouts, connection cap.

mod api;
mod http;
mod rng;

use http::{read_request, write_response, ParseError, Request, Response};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

const INDEX_HTML: &str = include_str!("../assets/index.html");
const APP_JS: &str = include_str!("../assets/app.js");
const APP_CSS: &str = include_str!("../assets/app.css");
const MAX_CONNECTIONS: usize = 32;
pub const COOKIE: &str = "kepler_ui";

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub db_path: PathBuf,
    /// 0 = a random free port.
    pub port: u16,
    /// Enable `POST /api/orbit/resolve` (approve/deny a waiting Orbit task).
    pub allow_actions: bool,
}

/// A running server. Dropping it does not stop it; use [`UiServer::shutdown`] or abort the task.
pub struct UiServer {
    pub addr: SocketAddr,
    pub token: String,
    pub allow_actions: bool,
    task: tokio::task::JoinHandle<()>,
}

impl UiServer {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?t={}", self.addr.port(), self.token)
    }

    pub fn shutdown(&self) {
        self.task.abort();
    }

    /// Wait until the server task ends (it normally never does).
    pub async fn wait(self) {
        let _ = self.task.await;
    }
}

struct State {
    cfg: UiConfig,
    token: String,
    port: u16,
}

pub async fn start(cfg: UiConfig) -> Result<UiServer, String> {
    let token = rng::new_token()?;
    let listener = TcpListener::bind(("127.0.0.1", cfg.port))
        .await
        .map_err(|e| format!("cannot bind 127.0.0.1:{}: {e}", cfg.port))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let allow_actions = cfg.allow_actions;
    let state = Arc::new(State {
        cfg,
        token: token.clone(),
        port: addr.port(),
    });
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, peer)) = listener.accept().await else {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            };
            // Defence in depth: the socket is bound to loopback, but refuse anything else.
            if !peer.ip().is_loopback() {
                continue;
            }
            let Ok(permit) = limit.clone().try_acquire_owned() else {
                let r = Response::text(503, "busy");
                write_response(&mut stream, &r, false).await;
                continue;
            };
            let st = state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let resp = match read_request(&mut stream).await {
                    Ok(req) => {
                        let head_only = req.method == "HEAD";
                        let r = handle(&st, &req).await;
                        write_response(&mut stream, &r, head_only).await;
                        return;
                    }
                    Err(ParseError::TooLarge) => Response::text(431, "request too large"),
                    Err(ParseError::Timeout) => Response::text(408, "timeout"),
                    Err(ParseError::Malformed) => Response::text(400, "bad request"),
                    Err(ParseError::Closed) => return,
                };
                write_response(&mut stream, &resp, false).await;
            });
        }
    });
    Ok(UiServer {
        addr,
        token,
        allow_actions,
        task,
    })
}

/// Constant-time equality for equal-length secrets (length is not secret here).
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|p| {
        let (k, v) = p.trim().split_once('=')?;
        (k == name).then_some(v)
    })
}

fn host_ok(req: &Request, port: u16) -> bool {
    match req.headers.get("host") {
        Some(h) => {
            let h = h.to_ascii_lowercase();
            h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}")
        }
        None => false,
    }
}

fn origin_ok(req: &Request, port: u16) -> bool {
    let same = |o: &str| {
        let o = o.to_ascii_lowercase();
        o == format!("http://127.0.0.1:{port}") || o == format!("http://localhost:{port}")
    };
    if let Some(o) = req.headers.get("origin") {
        if !same(o) {
            return false;
        }
    }
    if let Some(s) = req.headers.get("sec-fetch-site") {
        if !matches!(s.as_str(), "same-origin" | "none") {
            return false;
        }
    }
    true
}

fn authed(req: &Request, token: &str) -> bool {
    if let Some(c) = req.headers.get("cookie") {
        if cookie_value(c, COOKIE).is_some_and(|v| ct_eq(v, token)) {
            return true;
        }
    }
    req.headers
        .get("authorization")
        .and_then(|a| a.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t.trim(), token))
}

async fn handle(st: &State, req: &Request) -> Response {
    // 1. Anti DNS rebinding / wrong-origin: before anything else, including authentication.
    if !host_ok(req, st.port) {
        return Response::text(421, "unexpected Host header");
    }
    if !origin_ok(req, st.port) {
        return Response::text(403, "cross-origin request refused");
    }
    if !matches!(req.method.as_str(), "GET" | "HEAD" | "POST") {
        return Response::text(405, "method not allowed").header("Allow", "GET, HEAD, POST".into());
    }
    // 2. First visit with the token in the URL: set the cookie and drop the token from the URL.
    if req.method != "POST" && req.path == "/" {
        if let Some(t) = req.query.get("t") {
            if ct_eq(t, &st.token) {
                return Response::new(302, "text/plain; charset=utf-8", "")
                    .header("Location", "/".into())
                    .header(
                        "Set-Cookie",
                        format!("{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/", st.token),
                    );
            }
        }
    }
    // 3. Authentication for everything else.
    if !authed(req, &st.token) {
        return Response::text(
            401,
            "Kepler UI: open the URL printed by `kepler ui` (it carries the session token).",
        );
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET" | "HEAD", "/") => Response::new(200, "text/html; charset=utf-8", INDEX_HTML),
        ("GET" | "HEAD", "/app.js") => Response::new(200, "text/javascript; charset=utf-8", APP_JS),
        ("GET" | "HEAD", "/app.css") => Response::new(200, "text/css; charset=utf-8", APP_CSS),
        ("GET" | "HEAD", p) if p.starts_with("/api/") => api::get(&st.cfg, p, req).await,
        ("POST", "/api/orbit/resolve") => api::resolve(&st.cfg, req).await,
        ("POST", _) => Response::text(404, "not found"),
        _ => Response::text(404, "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_compare_and_cookie_parsing() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
        assert_eq!(
            cookie_value("a=1; kepler_ui=tok; b=2", "kepler_ui"),
            Some("tok")
        );
        assert_eq!(cookie_value("xkepler_ui=tok", "kepler_ui"), None);
    }
}
