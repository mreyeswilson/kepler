//! Daemon lifecycle: lazy start (by the first client), idle shutdown, no polling.

use crate::core::Core;
use crate::pipeline::{build_engine, PipelineConfig};
use kepler_ipc::{serve, Endpoint, Listener, ServeExit, ServerConfig};
use kepler_storage::{Writer, WriterConfig};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub endpoint: Endpoint,
    /// `None` disables persistence entirely.
    pub db_path: Option<PathBuf>,
    /// `None` = never shut down on idle.
    pub idle_shutdown: Option<Duration>,
    pub server: ServerConfig,
    pub pipeline: PipelineConfig,
}

impl DaemonConfig {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            db_path: None,
            idle_shutdown: Some(Duration::from_secs(300)),
            server: ServerConfig::default(),
            pipeline: PipelineConfig::default(),
        }
    }
}

#[derive(Debug)]
pub enum DaemonError {
    /// Another daemon already owns the endpoint (expected when two clients race to start one).
    AlreadyRunning,
    Io(std::io::Error),
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => f.write_str("a daemon is already running for this user"),
            Self::Io(e) => write!(f, "cannot start daemon: {e}"),
        }
    }
}

impl std::error::Error for DaemonError {}

pub struct Daemon {
    listener: Listener,
    core: Arc<Core>,
    cfg: DaemonConfig,
    /// Non-fatal problems at startup (e.g. storage unavailable). The daemon still works.
    pub warnings: Vec<String>,
}

impl Daemon {
    /// Bind the endpoint first (cheap, fails fast if another daemon exists),
    /// then open storage. A storage failure is a warning, never a startup failure.
    pub fn start(cfg: DaemonConfig) -> Result<Self, DaemonError> {
        let listener = Listener::bind(&cfg.endpoint).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                DaemonError::AlreadyRunning
            } else {
                DaemonError::Io(e)
            }
        })?;
        let mut warnings = Vec::new();
        let storage = match &cfg.db_path {
            None => None,
            Some(p) => match Writer::start(p, WriterConfig::default()) {
                Ok(w) => Some(Arc::new(w)),
                Err(e) => {
                    warnings.push(format!("persistence disabled: {e}"));
                    None
                }
            },
        };
        let memory_storage = match (&storage, &cfg.db_path) {
            (Some(w), Some(p)) => Some((w.clone(), p.clone())),
            _ => None,
        };
        let engine = build_engine(&cfg.pipeline, memory_storage);
        let core = Arc::new(Core::new(engine, storage));
        Ok(Self {
            listener,
            core,
            cfg,
            warnings,
        })
    }

    pub fn core(&self) -> Arc<Core> {
        self.core.clone()
    }

    /// Serve until `shutdown` resolves or the idle limit passes with no connections.
    pub async fn serve(self, shutdown: impl Future<Output = ()>) -> ServeExit {
        let mut server_cfg = self.cfg.server.clone();
        server_cfg.idle_shutdown = self.cfg.idle_shutdown;
        serve(self.listener, self.core, server_cfg, shutdown).await
    }
}
