//! Local IPC between harness adapters and the Kepler daemon.
//! Wire protocol: see `docs/decisions/0007-ipc-wire-protocol.md`.

pub mod client;
pub mod endpoint;
pub mod frame;
pub mod identity;
pub mod server;
pub mod transport;
pub mod wire;

pub use client::{Client, ClientError, HelloOptions};
pub use endpoint::Endpoint;
pub use server::{serve, BoxFuture, ConnectionId, Handler, Reject, ServeExit, ServerConfig};
pub use transport::{connect, IpcStream, Listener};
pub use wire::{ClientFrame, ServerFrame, MAX_FRAME_BYTES, PROTOCOL_VERSION};
