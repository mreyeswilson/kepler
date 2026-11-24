//! Local SQLite persistence with privacy defaults.
//!
//! * One dedicated writer thread (the single write coordinator) behind a bounded queue.
//! * Read-only connections for queries.
//! * Only event *metadata* is persisted: never prompts, command lines or tool output.
//! * Free text that is stored on purpose (memories) is redacted first, and sensitive
//!   paths (`.env`, keys, tokens...) are refused.

pub mod dirs;
mod error;
mod findings;
mod memory;
mod migrations;
mod orbit;
mod reader;
mod records;
pub mod redact {
    //! Redaction lives in `kepler-policy` (the Guardian also uses it); re-exported here.
    pub use kepler_policy::redact::*;
}
mod writer;

pub use error::StorageError;
pub use findings::{
    dismiss_finding, list_findings, store_findings, Confidence, FindingFilter, FindingRow,
    NewFinding, Severity, MAX_FINDINGS_PER_PROJECT,
};
pub use kepler_policy::sensitive::{is_sensitive_path, resolve_within, PathError};
pub use memory::{
    apply_retention, content_hash, fts_query, jaccard, mark_used, purge_memories, record_metric,
    set_setting, store_memory, tokens, Candidate, MemoryFilter, MemoryQuery, MemoryRow,
    MemoryStats, MetricRow, MetricSummary, NewMemory, SessionMetric, PurgeFilter, Scope, SourceRow, StoreOutcome,
    MAX_MEMORY_TEXT_BYTES, TRUST_DERIVED, TRUST_USER,
};
pub use migrations::{migrate, LATEST_VERSION};
pub use orbit::{
    create_run, delete_run, get_run, list_runs, load_tasks, resolve_approval, set_run_status,
    update_task, NewRun, NewTask, RunRow, TaskPatch, TaskRow, MAX_TASKS_PER_RUN,
};
pub use reader::{inspect, DbStatus, MemoryHit, Reader, StoredEvent};
pub use records::{EventRecord, MemoryRecord, MAX_MEMORY_BYTES};
pub use writer::{Writer, WriterConfig};

pub use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

/// Open the read/write connection with WAL and safe defaults, then migrate.
pub(crate) fn open_rw(path: &Path) -> Result<Connection, StorageError> {
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    migrate(&mut conn)?;
    restrict_file_permissions(path);
    Ok(conn)
}

/// Best effort 0600 on Unix. On Windows the per-user data directory ACL applies.
pub(crate) fn restrict_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}
