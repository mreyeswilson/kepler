//! Kepler Core: wires events, policy, storage and IPC into a lazy local daemon.
//!
//! Delivery 0 behavior: every event is validated, evaluated by the deterministic
//! policy engine (passthrough by default, fail-closed tool preflight), recorded as
//! metadata only, and answered. No model, browser or network is ever started.

mod core;
pub mod daemon;
pub mod doctor;
pub mod emit;
pub mod pipeline;
mod sessions;

pub use crate::core::Core;
pub use daemon::{Daemon, DaemonConfig, DaemonError};
pub use doctor::DoctorReport;
pub use emit::{emit_one_shot, EmitOptions, EmitOutcome};
pub use pipeline::{build_engine, PipelineConfig};
pub use sessions::{SessionRegistry, MAX_SESSIONS_PER_CONNECTION, MAX_TOTAL_SESSIONS};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
