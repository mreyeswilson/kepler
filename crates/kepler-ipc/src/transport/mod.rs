//! Platform transports: Unix domain socket (0600) / Windows named pipe (current-user ACL).

use crate::endpoint::Endpoint;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

pub trait AsyncStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> AsyncStream for T {}

/// A connected, type-erased duplex stream.
pub type IpcStream = Box<dyn AsyncStream>;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as imp;
#[cfg(unix)]
pub use unix::unix_socket_path;

#[cfg(windows)]
mod acl;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(not(any(unix, windows)))]
compile_error!("kepler-ipc supports only Unix and Windows");

/// Server side endpoint. Dropping it removes the Unix socket file.
pub struct Listener {
    inner: imp::Listener,
}

impl Listener {
    /// Bind the endpoint. Fails with `AddrInUse` if a live daemon already owns it.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        Ok(Self {
            inner: imp::Listener::bind(endpoint)?,
        })
    }

    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        self.inner.accept().await
    }
}

/// Connect to the daemon endpoint of the current user.
pub async fn connect(endpoint: &Endpoint) -> io::Result<IpcStream> {
    imp::connect(endpoint).await
}
