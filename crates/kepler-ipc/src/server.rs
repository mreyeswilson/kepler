//! Bounded, timeout-driven connection server. See ADR-0005 for the protocol.

use crate::frame::{read_frame, write_json, FrameError};
use crate::transport::{IpcStream, Listener};
use crate::wire::{
    ClientFrame, Hello, ServerFrame, MAX_FRAME_BYTES, MAX_REQUEST_ID_BYTES, PROTOCOL_VERSION,
};
use kepler_events::capabilities::negotiate;
use kepler_events::{is_valid_identifier, Envelope};
use kepler_policy::Verdict;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{split, BufReader};
use tokio::sync::{Notify, Semaphore};
use tokio::time::{timeout, Instant};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type ConnectionId = u64;

/// A handler-level refusal that becomes a non-fatal error frame.
#[derive(Debug, Clone)]
pub struct Reject {
    pub code: &'static str,
    pub message: String,
}

/// Application logic behind the transport (implemented by `kepler-core`).
pub trait Handler: Send + Sync + 'static {
    fn handle_event(
        &self,
        conn: ConnectionId,
        event: Envelope,
    ) -> BoxFuture<'_, Result<Verdict, Reject>>;

    /// Called exactly once when a connection ends (clean or not).
    fn connection_closed(&self, _conn: ConnectionId) {}
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub max_connections: usize,
    pub handshake_timeout: Duration,
    pub idle_timeout: Duration,
    pub write_timeout: Duration,
    pub handler_timeout: Duration,
    pub max_consecutive_errors: u32,
    /// Stop serving after this long with zero connections. `None` = run forever.
    pub idle_shutdown: Option<Duration>,
    pub server_version: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_connections: 32,
            handshake_timeout: Duration::from_secs(2),
            idle_timeout: Duration::from_secs(60),
            write_timeout: Duration::from_secs(5),
            handler_timeout: Duration::from_millis(500),
            max_consecutive_errors: 8,
            idle_shutdown: None,
            server_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeExit {
    IdleTimeout,
    Shutdown,
}

struct Shared {
    active: AtomicUsize,
    changed: Notify,
}

struct ActiveGuard<H: Handler> {
    shared: Arc<Shared>,
    handler: Arc<H>,
    id: ConnectionId,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl<H: Handler> Drop for ActiveGuard<H> {
    fn drop(&mut self) {
        self.handler.connection_closed(self.id);
        self.shared.active.fetch_sub(1, Ordering::SeqCst);
        self.shared.changed.notify_one();
    }
}

/// Accept connections until `shutdown` resolves or the idle limit is reached.
pub async fn serve<H: Handler>(
    mut listener: Listener,
    handler: Arc<H>,
    cfg: ServerConfig,
    shutdown: impl Future<Output = ()>,
) -> ServeExit {
    let cfg = Arc::new(cfg);
    let sem = Arc::new(Semaphore::new(cfg.max_connections.max(1)));
    let shared = Arc::new(Shared {
        active: AtomicUsize::new(0),
        changed: Notify::new(),
    });
    let mut next_id: ConnectionId = 1;
    let mut last_activity = Instant::now();
    tokio::pin!(shutdown);

    loop {
        let deadline = match cfg.idle_shutdown {
            Some(d) if shared.active.load(Ordering::SeqCst) == 0 => Some(last_activity + d),
            _ => None,
        };
        let idle = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            () = &mut shutdown => return ServeExit::Shutdown,
            () = idle => return ServeExit::IdleTimeout,
            () = shared.changed.notified() => { last_activity = Instant::now(); }
            accepted = listener.accept() => {
                last_activity = Instant::now();
                let stream = match accepted {
                    Ok(s) => s,
                    Err(_) => {
                        // Transient accept failure: back off instead of spinning.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let Ok(permit) = sem.clone().try_acquire_owned() else {
                    drop(stream); // at capacity: refuse immediately, no queueing
                    continue;
                };
                shared.active.fetch_add(1, Ordering::SeqCst);
                let id = next_id;
                next_id += 1;
                let guard = ActiveGuard {
                    shared: shared.clone(),
                    handler: handler.clone(),
                    id,
                    _permit: permit,
                };
                let cfg = cfg.clone();
                tokio::spawn(async move {
                    run_connection(stream, guard, &cfg).await;
                });
            }
        }
    }
}

enum Next {
    Frame(Vec<u8>),
    Closed,
    Fatal(ServerFrame),
}

async fn next_frame<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R, wait: Duration) -> Next {
    match timeout(wait, read_frame(r, MAX_FRAME_BYTES)).await {
        Err(_) => Next::Closed, // idle / handshake timeout: just drop
        Ok(Ok(Some(f))) => Next::Frame(f),
        Ok(Ok(None)) => Next::Closed,
        Ok(Err(FrameError::TooLarge)) => Next::Fatal(ServerFrame::error(
            None,
            "frame_too_large",
            format!("frame exceeds {MAX_FRAME_BYTES} bytes"),
            true,
        )),
        Ok(Err(_)) => Next::Closed,
    }
}

async fn send<W: tokio::io::AsyncWrite + Unpin>(
    w: &mut W,
    f: &ServerFrame,
    cfg: &ServerConfig,
) -> bool {
    matches!(
        timeout(cfg.write_timeout, write_json(w, f)).await,
        Ok(Ok(()))
    )
}

async fn run_connection<H: Handler>(stream: IpcStream, guard: ActiveGuard<H>, cfg: &ServerConfig) {
    let id = guard.id;
    let (r, mut w) = split(stream);
    let mut r = BufReader::with_capacity(8 * 1024, r);

    // ---- handshake
    let first = match next_frame(&mut r, cfg.handshake_timeout).await {
        Next::Frame(f) => f,
        Next::Closed => return,
        Next::Fatal(f) => {
            send(&mut w, &f, cfg).await;
            return;
        }
    };
    let hello: Hello = match serde_json::from_slice::<ClientFrame>(&first) {
        Ok(ClientFrame::Hello(h)) => h,
        _ => {
            let f = ServerFrame::error(
                None,
                "handshake_required",
                "first frame must be hello",
                true,
            );
            send(&mut w, &f, cfg).await;
            return;
        }
    };
    if hello.protocol != PROTOCOL_VERSION {
        let f = ServerFrame::error(
            None,
            "unsupported_protocol",
            format!(
                "server supports protocol {PROTOCOL_VERSION}, client sent {}",
                hello.protocol
            ),
            true,
        );
        send(&mut w, &f, cfg).await;
        return;
    }
    if hello.client.is_empty() || hello.client.len() > 64 {
        let f = ServerFrame::error(None, "bad_request", "invalid client name", true);
        send(&mut w, &f, cfg).await;
        return;
    }
    let negotiated = match &hello.capabilities {
        None => None,
        Some(adv) => match negotiate(adv) {
            Ok(n) => Some(n),
            Err(e) => {
                let f = ServerFrame::error(None, "invalid_capabilities", e.to_string(), true);
                send(&mut w, &f, cfg).await;
                return;
            }
        },
    };
    let ack = ServerFrame::HelloAck {
        protocol: PROTOCOL_VERSION,
        server_version: cfg.server_version.clone(),
        max_frame_bytes: MAX_FRAME_BYTES,
        negotiated,
    };
    if !send(&mut w, &ack, cfg).await {
        return;
    }

    // ---- request loop (one in-flight request per connection = natural backpressure)
    let mut errors = 0u32;
    loop {
        let bytes = match next_frame(&mut r, cfg.idle_timeout).await {
            Next::Frame(f) => f,
            Next::Closed => return,
            Next::Fatal(f) => {
                send(&mut w, &f, cfg).await;
                return;
            }
        };
        let reply = respond(&bytes, id, &guard.handler, cfg).await;
        let is_error = matches!(reply, ServerFrame::Error { .. });
        if is_error {
            errors += 1;
        } else {
            errors = 0;
        }
        if !send(&mut w, &reply, cfg).await {
            return;
        }
        if errors >= cfg.max_consecutive_errors {
            let f = ServerFrame::error(None, "too_many_errors", "closing connection", true);
            send(&mut w, &f, cfg).await;
            return;
        }
        if matches!(reply, ServerFrame::Error { fatal: true, .. }) {
            return;
        }
    }
}

async fn respond<H: Handler>(
    bytes: &[u8],
    conn: ConnectionId,
    handler: &Arc<H>,
    cfg: &ServerConfig,
) -> ServerFrame {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return ServerFrame::error(None, "empty_frame", "empty frame", false);
    }
    let frame: ClientFrame = match serde_json::from_slice(bytes) {
        Ok(f) => f,
        Err(e) => {
            return ServerFrame::error(None, "bad_frame", format!("cannot parse frame: {e}"), false)
        }
    };
    match frame {
        ClientFrame::Hello(_) => {
            ServerFrame::error(None, "bad_request", "hello already completed", false)
        }
        ClientFrame::Ping { request_id } => {
            if request_id
                .as_deref()
                .is_some_and(|r| !is_valid_identifier(r, MAX_REQUEST_ID_BYTES))
            {
                return ServerFrame::error(None, "bad_request", "invalid request_id", false);
            }
            ServerFrame::Pong { request_id }
        }
        ClientFrame::Event { request_id, event } => {
            if !is_valid_identifier(&request_id, MAX_REQUEST_ID_BYTES) {
                return ServerFrame::error(None, "bad_request", "invalid request_id", false);
            }
            let env = match Envelope::from_value(event) {
                Ok(e) => e,
                Err(e) => {
                    return ServerFrame::error(Some(request_id), e.code(), e.to_string(), false)
                }
            };
            let event_id = env.id.clone();
            match timeout(cfg.handler_timeout, handler.handle_event(conn, env)).await {
                Err(_) => ServerFrame::error(
                    Some(request_id),
                    "handler_timeout",
                    "handler exceeded its time budget",
                    false,
                ),
                Ok(Err(rej)) => ServerFrame::error(Some(request_id), rej.code, rej.message, false),
                Ok(Ok(decision)) => ServerFrame::Decision {
                    request_id,
                    event_id,
                    decision,
                },
            }
        }
    }
}
