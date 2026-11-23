//! Windows named pipe transport with a current-user-only DACL.

use super::{acl, IpcStream};
use crate::endpoint::Endpoint;
use std::io;
use std::time::Duration;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

const ERROR_PIPE_BUSY: i32 = 231;

fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first).reject_remote_clients(true);
    acl::create_user_only_pipe(&opts, name)
}

pub struct Listener {
    name: String,
    server: NamedPipeServer,
}

impl Listener {
    pub fn bind(ep: &Endpoint) -> io::Result<Self> {
        let name = ep.pipe_name();
        // first_pipe_instance(true) fails with access denied if the name already
        // exists (live daemon, or a squatter): never adopt an existing pipe.
        let server = create(&name, true).map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "daemon already running or pipe taken",
                )
            } else {
                e
            }
        })?;
        Ok(Self { name, server })
    }

    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        self.server.connect().await?;
        let next = create(&self.name, false)?;
        let connected = std::mem::replace(&mut self.server, next);
        Ok(Box::new(connected))
    }
}

pub async fn connect(ep: &Endpoint) -> io::Result<IpcStream> {
    let name = ep.pipe_name();
    // All instances may be momentarily busy under load: retry briefly.
    for _ in 0..20 {
        match ClientOptions::new().open(&name) {
            Ok(c) => return Ok(Box::new(c)),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "pipe busy"))
}
