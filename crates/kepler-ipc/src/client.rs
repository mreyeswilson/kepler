//! Reference client (used by the CLI and tests). Adapters in other languages
//! implement the same line-delimited JSON protocol.

use crate::endpoint::Endpoint;
use crate::frame::{read_frame, write_json, FrameError};
use crate::transport::{connect, IpcStream};
use crate::wire::{ClientFrame, Hello, ServerFrame, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use kepler_events::capabilities::{Advertisement, Negotiated};
use kepler_events::Envelope;
use kepler_policy::Verdict;
use std::io;
use std::time::Duration;
use tokio::io::{split, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::time::timeout;

#[derive(Debug)]
pub enum ClientError {
    /// Could not connect (daemon not running, permission, busy). Adapters must pass through.
    Unavailable(io::Error),
    Timeout,
    Closed,
    Protocol(String),
    /// The server answered with an error frame.
    Rejected {
        code: String,
        message: String,
        fatal: bool,
    },
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(e) => write!(f, "daemon unavailable: {e}"),
            Self::Timeout => f.write_str("timed out"),
            Self::Closed => f.write_str("connection closed"),
            Self::Protocol(m) => write!(f, "protocol error: {m}"),
            Self::Rejected { code, message, .. } => write!(f, "rejected ({code}): {message}"),
        }
    }
}

impl std::error::Error for ClientError {}

#[derive(Debug, Clone)]
pub struct HelloOptions {
    pub client: String,
    pub client_version: Option<String>,
    pub capabilities: Option<Advertisement>,
    pub timeout: Duration,
}

impl HelloOptions {
    pub fn new(client: &str) -> Self {
        Self {
            client: client.to_string(),
            client_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            capabilities: None,
            timeout: Duration::from_secs(2),
        }
    }
}

pub struct Client {
    r: BufReader<ReadHalf<IpcStream>>,
    w: WriteHalf<IpcStream>,
    timeout: Duration,
    next: u64,
    pub server_version: String,
    pub negotiated: Option<Negotiated>,
}

impl Client {
    /// Connect and complete the handshake, all within `opts.timeout`.
    pub async fn connect(endpoint: &Endpoint, opts: HelloOptions) -> Result<Self, ClientError> {
        let stream = timeout(opts.timeout, connect(endpoint))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(ClientError::Unavailable)?;
        Self::handshake(stream, opts).await
    }

    /// Handshake over an already connected stream (also used by tests).
    pub async fn handshake(stream: IpcStream, opts: HelloOptions) -> Result<Self, ClientError> {
        let (r, w) = split(stream);
        let mut c = Self {
            r: BufReader::new(r),
            w,
            timeout: opts.timeout,
            next: 1,
            server_version: String::new(),
            negotiated: None,
        };
        let hello = ClientFrame::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            client: opts.client,
            client_version: opts.client_version,
            capabilities: opts.capabilities,
        });
        c.send(&hello).await?;
        match c.recv().await? {
            ServerFrame::HelloAck {
                server_version,
                negotiated,
                ..
            } => {
                c.server_version = server_version;
                c.negotiated = negotiated;
                Ok(c)
            }
            other => Err(unexpected(other)),
        }
    }

    pub async fn send(&mut self, frame: &ClientFrame) -> Result<(), ClientError> {
        timeout(self.timeout, write_json(&mut self.w, frame))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::Closed)
    }

    /// Write raw bytes (tests use this to send malformed frames).
    pub async fn send_raw(&mut self, bytes: &[u8]) -> Result<(), ClientError> {
        let res = async {
            self.w.write_all(bytes).await?;
            self.w.flush().await
        };
        timeout(self.timeout, res)
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::Closed)
    }

    pub async fn recv(&mut self) -> Result<ServerFrame, ClientError> {
        let bytes = match timeout(self.timeout, read_frame(&mut self.r, MAX_FRAME_BYTES)).await {
            Err(_) => return Err(ClientError::Timeout),
            Ok(Ok(Some(b))) => b,
            Ok(Ok(None)) | Ok(Err(FrameError::Truncated)) => return Err(ClientError::Closed),
            Ok(Err(e)) => return Err(ClientError::Protocol(e.to_string())),
        };
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    fn request_id(&mut self) -> String {
        let id = format!("r{}", self.next);
        self.next += 1;
        id
    }

    /// Send an event and wait for its verdict.
    pub async fn send_event(&mut self, event: &Envelope) -> Result<Verdict, ClientError> {
        let value =
            serde_json::to_value(event).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.send_event_value(value).await
    }

    /// Like [`Client::send_event`] for an unvalidated JSON value.
    pub async fn send_event_value(
        &mut self,
        event: serde_json::Value,
    ) -> Result<Verdict, ClientError> {
        let request_id = self.request_id();
        self.send(&ClientFrame::Event {
            request_id: request_id.clone(),
            event,
        })
        .await?;
        match self.recv().await? {
            ServerFrame::Decision {
                request_id: rid,
                decision,
                ..
            } if rid == request_id => Ok(decision),
            other => Err(unexpected(other)),
        }
    }

    pub async fn ping(&mut self) -> Result<(), ClientError> {
        let request_id = Some(self.request_id());
        self.send(&ClientFrame::Ping {
            request_id: request_id.clone(),
        })
        .await?;
        match self.recv().await? {
            ServerFrame::Pong { request_id: rid } if rid == request_id => Ok(()),
            other => Err(unexpected(other)),
        }
    }
}

fn unexpected(f: ServerFrame) -> ClientError {
    match f {
        ServerFrame::Error {
            code,
            message,
            fatal,
            ..
        } => ClientError::Rejected {
            code,
            message,
            fatal,
        },
        other => ClientError::Protocol(format!("unexpected frame: {other:?}")),
    }
}
