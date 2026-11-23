#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use kepler_events::capabilities::{Advertisement, Capability, CapabilityEntry, ImplKind};
use kepler_events::Envelope;
use kepler_ipc::{
    serve, BoxFuture, Client, ClientError, ClientFrame, ConnectionId, Endpoint, Handler,
    HelloOptions, Listener, Reject, ServeExit, ServerConfig, ServerFrame,
};
use kepler_policy::{Decision, Verdict};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_endpoint() -> Endpoint {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    Endpoint::new(&format!("t{}-{}", std::process::id(), n)).unwrap()
}

#[derive(Default)]
struct TestHandler {
    events: AtomicUsize,
    closed: AtomicUsize,
    delay: Duration,
}

impl Handler for TestHandler {
    fn handle_event(
        &self,
        _c: ConnectionId,
        _e: Envelope,
    ) -> BoxFuture<'_, Result<Verdict, Reject>> {
        Box::pin(async move {
            self.events.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(Verdict::new(Decision::passthrough("test")))
        })
    }
    fn connection_closed(&self, _c: ConnectionId) {
        self.closed.fetch_add(1, Ordering::SeqCst);
    }
}

fn fast_cfg() -> ServerConfig {
    ServerConfig {
        handshake_timeout: Duration::from_millis(300),
        idle_timeout: Duration::from_secs(5),
        handler_timeout: Duration::from_millis(300),
        ..ServerConfig::default()
    }
}

struct Running {
    ep: Endpoint,
    handler: Arc<TestHandler>,
    task: JoinHandle<ServeExit>,
    stop: tokio::sync::oneshot::Sender<()>,
}

fn start(cfg: ServerConfig, handler: TestHandler) -> Running {
    let ep = unique_endpoint();
    let listener = Listener::bind(&ep).unwrap();
    let handler = Arc::new(handler);
    let (stop, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(serve(listener, handler.clone(), cfg, async move {
        let _ = rx.await;
    }));
    Running {
        ep,
        handler,
        task,
        stop,
    }
}

fn event_json(session: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 1, "id": "e1", "type": "input.received",
        "timestamp": "2026-01-02T03:04:05Z", "harness": "pi",
        "project_id": "p", "session_id": session, "correlation_id": "c",
        "payload": { "text": "Fix the failing test in parser.rs" }
    })
}

fn event() -> Envelope {
    Envelope::from_value(event_json("s1")).unwrap()
}

async fn client(ep: &Endpoint) -> Client {
    Client::connect(ep, HelloOptions::new("test"))
        .await
        .unwrap()
}

#[tokio::test]
async fn handshake_event_roundtrip_and_ping() {
    let s = start(fast_cfg(), TestHandler::default());
    let mut c = client(&s.ep).await;
    assert!(!c.server_version.is_empty());
    c.ping().await.unwrap();
    let v = c.send_event(&event()).await.unwrap();
    assert_eq!(v.decision, Decision::passthrough("test"));
    assert_eq!(s.handler.events.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn hello_negotiates_capabilities() {
    let s = start(fast_cfg(), TestHandler::default());
    let mut opts = HelloOptions::new("pi-adapter");
    opts.capabilities = Some(Advertisement {
        harness: "pi".into(),
        harness_version: None,
        capabilities: vec![CapabilityEntry {
            capability: "input_transform".into(),
            kind: ImplKind::McpTool,
            version: None,
            constraints: vec![],
        }],
    });
    let c = Client::connect(&s.ep, opts).await.unwrap();
    let n = c.negotiated.unwrap();
    assert!(
        !n.is_granted(Capability::InputTransform),
        "MCP must not grant interception"
    );
}

#[tokio::test]
async fn unsupported_protocol_is_fatal() {
    let s = start(fast_cfg(), TestHandler::default());
    let stream = kepler_ipc::connect(&s.ep).await.unwrap();
    let (r, mut w) = tokio::io::split(stream);
    let mut r = tokio::io::BufReader::new(r);
    kepler_ipc::frame::write_json(
        &mut w,
        &serde_json::json!({"type":"hello","protocol":99,"client":"x"}),
    )
    .await
    .unwrap();
    let f = kepler_ipc::frame::read_frame(&mut r, 65536)
        .await
        .unwrap()
        .unwrap();
    match serde_json::from_slice::<ServerFrame>(&f).unwrap() {
        ServerFrame::Error { code, fatal, .. } => {
            assert_eq!(code, "unsupported_protocol");
            assert!(fatal);
        }
        other => panic!("{other:?}"),
    }
    assert!(kepler_ipc::frame::read_frame(&mut r, 65536)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn event_before_hello_is_rejected() {
    let s = start(fast_cfg(), TestHandler::default());
    let stream = kepler_ipc::connect(&s.ep).await.unwrap();
    let (r, mut w) = tokio::io::split(stream);
    let mut r = tokio::io::BufReader::new(r);
    kepler_ipc::frame::write_json(
        &mut w,
        &ClientFrame::Event {
            request_id: "r1".into(),
            event: event_json("s1"),
        },
    )
    .await
    .unwrap();
    let f = kepler_ipc::frame::read_frame(&mut r, 65536)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<ServerFrame>(&f).unwrap(),
        ServerFrame::Error { fatal: true, .. }
    ));
    assert_eq!(s.handler.events.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_frame_is_fatal_and_server_survives() {
    let s = start(fast_cfg(), TestHandler::default());
    let mut c = client(&s.ep).await;
    let mut big = vec![b'a'; kepler_ipc::MAX_FRAME_BYTES + 10];
    big.push(b'\n');
    c.send_raw(&big).await.unwrap();
    match c.recv().await.unwrap() {
        ServerFrame::Error { code, fatal, .. } => {
            assert_eq!(code, "frame_too_large");
            assert!(fatal);
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(c.recv().await, Err(ClientError::Closed)));
    // The daemon is still healthy for new clients.
    client(&s.ep).await.ping().await.unwrap();
}

#[tokio::test]
async fn malformed_frames_are_nonfatal_until_too_many() {
    let s = start(fast_cfg(), TestHandler::default());
    let mut c = client(&s.ep).await;
    c.send_raw(b"{not json\n").await.unwrap();
    assert!(
        matches!(c.recv().await.unwrap(), ServerFrame::Error { code, fatal: false, .. } if code == "bad_frame")
    );
    c.send_raw(b"\n").await.unwrap();
    assert!(
        matches!(c.recv().await.unwrap(), ServerFrame::Error { code, .. } if code == "empty_frame")
    );
    c.send_raw(b"{\"type\":\"nope\"}\n").await.unwrap();
    assert!(
        matches!(c.recv().await.unwrap(), ServerFrame::Error { code, .. } if code == "bad_frame")
    );
    // still usable after errors, and success resets the counter
    c.ping().await.unwrap();
    // 8 consecutive errors close the connection
    let mut fatal_seen = false;
    for _ in 0..10 {
        if c.send_raw(b"garbage\n").await.is_err() {
            break;
        }
        match c.recv().await {
            Ok(ServerFrame::Error { code, .. }) if code == "too_many_errors" => {
                fatal_seen = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(fatal_seen);
    assert_eq!(s.handler.events.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_events_get_validation_codes_and_never_reach_the_handler() {
    let s = start(fast_cfg(), TestHandler::default());
    let mut c = client(&s.ep).await;
    let mut bad_version = event_json("s1");
    bad_version["version"] = 2.into();
    let err = c.send_event_value(bad_version).await.unwrap_err();
    assert!(
        matches!(err, ClientError::Rejected { ref code, fatal: false, .. } if code == "unsupported_version")
    );
    let err = c
        .send_event_value(event_json("../../etc/passwd"))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Rejected { ref code, .. } if code == "invalid_field"));
    let err = c
        .send_event_value(serde_json::json!([1, 2]))
        .await
        .unwrap_err();
    assert!(matches!(err, ClientError::Rejected { ref code, .. } if code == "malformed"));
    assert_eq!(s.handler.events.load(Ordering::SeqCst), 0);
    // connection still fine
    c.send_event(&event()).await.unwrap();
}

#[tokio::test]
async fn handshake_timeout_closes_silent_connections() {
    let s = start(fast_cfg(), TestHandler::default());
    let stream = kepler_ipc::connect(&s.ep).await.unwrap();
    let (r, _w) = tokio::io::split(stream);
    let mut r = tokio::io::BufReader::new(r);
    let res = tokio::time::timeout(
        Duration::from_secs(3),
        kepler_ipc::frame::read_frame(&mut r, 1024),
    )
    .await
    .expect("server must close, not hang");
    assert!(matches!(res, Ok(None) | Err(_)));
}

#[tokio::test]
async fn slow_handler_yields_handler_timeout_error() {
    let h = TestHandler {
        delay: Duration::from_secs(2),
        ..Default::default()
    };
    let s = start(fast_cfg(), h);
    let mut c = client(&s.ep).await;
    let err = c.send_event(&event()).await.unwrap_err();
    assert!(matches!(err, ClientError::Rejected { ref code, .. } if code == "handler_timeout"));
}

#[tokio::test]
async fn flood_of_pipelined_requests_is_all_answered_in_order() {
    use tokio::io::AsyncWriteExt;
    let s = start(fast_cfg(), TestHandler::default());
    let stream = kepler_ipc::connect(&s.ep).await.unwrap();
    let (r, mut w) = tokio::io::split(stream);
    let mut r = tokio::io::BufReader::new(r);
    kepler_ipc::frame::write_json(
        &mut w,
        &serde_json::json!({"type":"hello","protocol":1,"client":"flood"}),
    )
    .await
    .unwrap();
    let ack = kepler_ipc::frame::read_frame(&mut r, 65536)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<ServerFrame>(&ack).unwrap(),
        ServerFrame::HelloAck { .. }
    ));
    let n = 3000usize;
    let mut payload = Vec::new();
    for i in 0..n {
        payload.extend_from_slice(
            format!("{{\"type\":\"ping\",\"request_id\":\"p{i}\"}}\n").as_bytes(),
        );
    }
    // Writer runs concurrently with the reader: the server must apply
    // backpressure through the transport, not buffer without bound or die.
    let writer = tokio::spawn(async move {
        w.write_all(&payload).await.unwrap();
        w.flush().await.unwrap();
        w
    });
    for got in 0..n {
        let f = kepler_ipc::frame::read_frame(&mut r, 65536)
            .await
            .unwrap()
            .unwrap();
        match serde_json::from_slice::<ServerFrame>(&f).unwrap() {
            ServerFrame::Pong { request_id } => {
                assert_eq!(request_id.as_deref(), Some(format!("p{got}").as_str()));
            }
            other => panic!("{other:?}"),
        }
    }
    let _w = writer.await.unwrap();
}

#[tokio::test]
async fn connection_flood_beyond_limit_is_refused_and_server_recovers() {
    let cfg = ServerConfig {
        max_connections: 4,
        ..fast_cfg()
    };
    let s = start(cfg, TestHandler::default());
    // Hold 4 connections open.
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(client(&s.ep).await);
    }
    // 40 more must not be served (closed without handshake) and must not hang the server.
    let mut refused = 0;
    for _ in 0..40 {
        if let Ok(stream) = kepler_ipc::connect(&s.ep).await {
            let (r, mut w) = tokio::io::split(stream);
            let mut r = tokio::io::BufReader::new(r);
            let _ = kepler_ipc::frame::write_json(
                &mut w,
                &serde_json::json!({"type":"hello","protocol":1,"client":"x"}),
            )
            .await;
            match tokio::time::timeout(
                Duration::from_secs(2),
                kepler_ipc::frame::read_frame(&mut r, 4096),
            )
            .await
            {
                Ok(Ok(None)) | Ok(Err(_)) => refused += 1,
                _ => {}
            }
        } else {
            refused += 1;
        }
    }
    assert_eq!(refused, 40);
    for c in &mut held {
        c.ping().await.unwrap();
    }
    drop(held);
    tokio::time::sleep(Duration::from_millis(200)).await;
    client(&s.ep).await.ping().await.unwrap();
}

#[tokio::test]
async fn connection_closed_callback_fires_once_per_connection() {
    let s = start(fast_cfg(), TestHandler::default());
    for _ in 0..3 {
        let mut c = client(&s.ep).await;
        c.ping().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(s.handler.closed.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn idle_shutdown_triggers_only_without_connections() {
    let cfg = ServerConfig {
        idle_shutdown: Some(Duration::from_millis(400)),
        ..fast_cfg()
    };
    let s = start(cfg, TestHandler::default());
    let mut c = client(&s.ep).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert!(
        !s.task.is_finished(),
        "must stay up while a connection is open"
    );
    c.ping().await.unwrap();
    drop(c);
    let exit = tokio::time::timeout(Duration::from_secs(3), s.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, ServeExit::IdleTimeout);
}

#[tokio::test]
async fn shutdown_signal_stops_server() {
    let s = start(fast_cfg(), TestHandler::default());
    s.stop.send(()).unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(2), s.task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exit, ServeExit::Shutdown);
}

#[tokio::test]
async fn second_listener_on_same_endpoint_is_refused() {
    let s = start(fast_cfg(), TestHandler::default());
    let err = Listener::bind(&s.ep).err().expect("must not bind twice");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
}

#[tokio::test]
async fn connecting_without_daemon_reports_unavailable() {
    let ep = unique_endpoint();
    let err = Client::connect(&ep, HelloOptions::new("t"))
        .await
        .err()
        .unwrap();
    assert!(matches!(err, ClientError::Unavailable(_)), "{err}");
}
