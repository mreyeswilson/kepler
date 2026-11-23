//! Unix domain socket transport. Not compiled on Windows; exercised by the CI matrix.

use super::IpcStream;
use crate::endpoint::Endpoint;
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

fn current_uid() -> io::Result<u32> {
    // No libc dependency: the owner of our own /proc entry (Linux) or of $HOME.
    if let Ok(m) = std::fs::metadata("/proc/self") {
        return Ok(m.uid());
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::other("cannot determine current user"))?;
    Ok(std::fs::metadata(home)?.uid())
}

fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("kepler"),
        None => std::env::temp_dir().join(format!("kepler-{}", Endpoint::user_tag())),
    }
}

pub fn unix_socket_path(ep: &Endpoint) -> io::Result<PathBuf> {
    Ok(runtime_dir().join(format!("kepler-{}.sock", ep.name())))
}

/// Create the runtime dir with mode 0700 and refuse it unless it is a real
/// directory owned by us and not accessible to group/others.
fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::other("runtime path is not a directory"));
    }
    if meta.uid() != current_uid()? {
        return Err(io::Error::other(
            "runtime directory is not owned by the current user",
        ));
    }
    if meta.permissions().mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
}

impl Listener {
    pub fn bind(ep: &Endpoint) -> io::Result<Self> {
        let path = unix_socket_path(ep)?;
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("bad socket path"))?;
        ensure_private_dir(dir)?;
        if path.exists() {
            // Live daemon? then refuse; otherwise it is a stale file from a crash.
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "daemon already running",
                ));
            }
            std::fs::remove_file(&path)?;
        }
        let inner = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self { inner, path })
    }

    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        let (s, _) = self.inner.accept().await?;
        Ok(Box::new(s))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn connect(ep: &Endpoint) -> io::Result<IpcStream> {
    let s = UnixStream::connect(unix_socket_path(ep)?).await?;
    Ok(Box::new(s))
}
