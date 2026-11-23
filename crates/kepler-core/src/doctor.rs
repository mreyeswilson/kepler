//! `kepler doctor`: honest, read-only diagnostics. Never creates files, never starts a daemon.

use kepler_events::capabilities::{cli_only, negotiate, Capability};
use kepler_events::{EventKind, ENVELOPE_VERSION, MAX_ENVELOPE_BYTES};
use kepler_ipc::{Client, Endpoint, HelloOptions, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use kepler_storage::{inspect, DbStatus};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

pub struct DoctorReport {
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub endpoint: String,
    pub daemon: DaemonProbe,
    pub database: DbStatus,
    pub data_dir_error: Option<String>,
    /// Memory counters when the database exists and has the v2 schema.
    pub memory: Option<(kepler_storage::MemoryStats, bool)>,
}

pub enum DaemonProbe {
    Running { server_version: String },
    NotRunning(String),
}

/// Probe the daemon (short timeout, no autostart) and inspect the database file.
pub async fn run(endpoint: &Endpoint, db_path: Result<&Path, String>) -> DoctorReport {
    let mut hello = HelloOptions::new("kepler-doctor");
    hello.timeout = Duration::from_millis(500);
    let daemon = match Client::connect(endpoint, hello).await {
        Ok(mut c) => match c.ping().await {
            Ok(()) => DaemonProbe::Running {
                server_version: c.server_version.clone(),
            },
            Err(e) => DaemonProbe::NotRunning(format!("connected but ping failed: {e}")),
        },
        Err(e) => DaemonProbe::NotRunning(e.to_string()),
    };
    let mut memory = None;
    let (database, data_dir_error) = match db_path {
        Ok(p) => {
            let st = inspect(p);
            if st.exists && st.error.is_none() {
                let admin = kepler_memory::MemoryAdmin::new(p);
                memory = match (admin.stats(None), admin.is_enabled(None)) {
                    (Ok(s), Ok(e)) => Some((s, e)),
                    _ => None,
                };
            }
            (st, None)
        }
        Err(e) => (
            DbStatus {
                path: String::new(),
                exists: false,
                schema_version: None,
                journal_mode: None,
                fts5: None,
                size_bytes: None,
                error: None,
            },
            Some(e),
        ),
    };
    DoctorReport {
        version: crate::VERSION,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        endpoint: endpoint.describe(),
        daemon,
        database,
        data_dir_error,
        memory,
    }
}

impl DoctorReport {
    pub fn to_json(&self) -> Value {
        let fallback = negotiate(&cli_only("cli")).ok();
        let cli_granted: Vec<&str> = Capability::ALL
            .iter()
            .filter(|c| fallback.as_ref().is_some_and(|n| n.is_granted(**c)))
            .map(|c| c.as_str())
            .collect();
        let cli_unsupported: Vec<&str> = Capability::ALL
            .iter()
            .filter(|c| !fallback.as_ref().is_some_and(|n| n.is_granted(**c)))
            .map(|c| c.as_str())
            .collect();
        json!({
            "kepler": { "version": self.version, "os": self.os, "arch": self.arch },
            "runtime": {
                "async": "tokio (current-thread in daemon)",
                "protocol": PROTOCOL_VERSION,
                "event_schema": ENVELOPE_VERSION,
                "event_kinds": EventKind::ALL.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
                "limits": { "max_frame_bytes": MAX_FRAME_BYTES, "max_envelope_bytes": MAX_ENVELOPE_BYTES },
                "llm_or_network_calls": false,
            },
            "ipc": {
                "endpoint": self.endpoint,
                "daemon": match &self.daemon {
                    DaemonProbe::Running { server_version } => json!({"running": true, "version": server_version}),
                    DaemonProbe::NotRunning(why) => json!({"running": false, "detail": why}),
                },
            },
            "database": {
                "path": self.database.path,
                "exists": self.database.exists,
                "schema_version": self.database.schema_version,
                "latest_schema_version": kepler_storage::LATEST_VERSION,
                "journal_mode": self.database.journal_mode,
                "fts5": self.database.fts5,
                "size_bytes": self.database.size_bytes,
                "error": self.database.error.clone().or_else(|| self.data_dir_error.clone()),
            },
            "capabilities": {
                "contract": Capability::ALL.iter().map(|c| json!({
                    "name": c.as_str(),
                    "requires_native_hook": c.requires_native_hook(),
                })).collect::<Vec<_>>(),
                "cli_only_fallback": { "granted": cli_granted, "unsupported": cli_unsupported },
                "note": "MCP/CLI never provide transparent interception; only native hooks do.",
            },
            "modules": {
                "pipeline_order": ["original (never modified)", "forge", "memory", "guardian", "harness"],
                "forge": { "status": "implemented", "disable": "KEPLER_FORGE=off", "calls_model": false, "model_assist": "adapter side, see forge_model" },
                "memory": {
                    "status": "implemented",
                    "disable": "KEPLER_MEMORY=off or `kepler memory disable`",
                    "global_enabled": self.memory.as_ref().map(|(_, e)| *e),
                    "total": self.memory.as_ref().map(|(s, _)| s.total),
                    "live": self.memory.as_ref().map(|(s, _)| s.live),
                },
            },
            "adapters": [
                { "harness": "pi", "status": "hooks_unverified",
                  "detail": "adapters/pi is private; not verified against a live Pi session" },
                { "harness": "claude-code|gemini-cli|opencode|codex", "status": "not_implemented" }
            ],
        })
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let line = |s: &mut String, k: &str, v: String| {
            s.push_str(&format!("{k:<22}{v}\n"));
        };
        s.push_str(&format!(
            "kepler {} ({} {})\n\n",
            self.version, self.os, self.arch
        ));
        s.push_str("Runtime\n");
        line(&mut s, "  protocol", PROTOCOL_VERSION.to_string());
        line(&mut s, "  event schema", ENVELOPE_VERSION.to_string());
        line(&mut s, "  max frame", format!("{MAX_FRAME_BYTES} bytes"));
        line(&mut s, "  llm/network calls", "none".into());
        s.push_str("\nIPC\n");
        line(&mut s, "  endpoint", self.endpoint.clone());
        match &self.daemon {
            DaemonProbe::Running { server_version } => {
                line(&mut s, "  daemon", format!("running (v{server_version})"))
            }
            DaemonProbe::NotRunning(why) => line(
                &mut s,
                "  daemon",
                format!("not running ({why}); starts lazily on first event"),
            ),
        }
        s.push_str("\nDatabase\n");
        let d = &self.database;
        line(&mut s, "  path", d.path.clone());
        if let Some(e) = d.error.as_ref().or(self.data_dir_error.as_ref()) {
            line(&mut s, "  status", format!("ERROR: {e}"));
        } else if !d.exists {
            line(
                &mut s,
                "  status",
                "not created yet (created when the daemon first runs)".into(),
            );
        } else {
            line(
                &mut s,
                "  schema",
                format!(
                    "v{} (latest v{})",
                    d.schema_version.unwrap_or(0),
                    kepler_storage::LATEST_VERSION
                ),
            );
            line(
                &mut s,
                "  journal mode",
                d.journal_mode.clone().unwrap_or_default(),
            );
            line(&mut s, "  fts5", format!("{}", d.fts5.unwrap_or(false)));
            line(
                &mut s,
                "  size",
                format!("{} bytes", d.size_bytes.unwrap_or(0)),
            );
        }
        s.push_str("\nCapabilities (contract)\n");
        for c in Capability::ALL {
            let need = if c.requires_native_hook() {
                "native hook only"
            } else {
                "native hook, MCP or CLI"
            };
            line(&mut s, &format!("  {}", c.as_str()), need.into());
        }
        s.push_str("\nModules (order: original -> forge -> memory -> guardian -> harness)\n");
        line(
            &mut s,
            "  forge",
            "implemented (KEPLER_FORGE=off disables)".into(),
        );
        match &self.memory {
            Some((m, enabled)) => line(
                &mut s,
                "  memory",
                format!(
                    "{} | {} live of {} stored",
                    if *enabled { "enabled" } else { "DISABLED" },
                    m.live,
                    m.total
                ),
            ),
            None => line(&mut s, "  memory", "implemented (no database yet)".into()),
        }
        s.push_str("\nAdapters\n");
        line(
            &mut s,
            "  pi",
            "automatic hooks (private, not verified with a live Pi)".into(),
        );
        line(&mut s, "  others", "not implemented".into());
        s
    }
}
