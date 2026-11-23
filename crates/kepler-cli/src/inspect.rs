//! `kepler inspect` and the data behind the MCP tools. Read-only: never creates the database,
//! never starts a daemon, never returns prompts or tool output (none are stored).

use kepler_ipc::Endpoint;
use kepler_memory::MemoryAdmin;
use kepler_storage::Reader;
use serde_json::{json, Value};

pub fn db_path() -> Result<std::path::PathBuf, String> {
    kepler_storage::dirs::db_path().map_err(|e| e.to_string())
}

/// Recent event metadata, memory counters and token metrics (each labeled estimated/measured).
pub fn inspect_value(limit: u32) -> Result<Value, String> {
    let path = db_path()?;
    if !path.exists() {
        return Ok(json!({
            "database": {"path": path.display().to_string(), "exists": false},
            "note": "nothing recorded yet; the database is created when the daemon first runs",
        }));
    }
    let reader = Reader::open(&path).map_err(|e| e.to_string())?;
    let events: Vec<Value> = reader
        .recent_events(limit)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|e| {
            json!({
                "event_id": e.event_id, "kind": e.kind, "session_id": e.session_id,
                "decision": e.decision, "reason": e.reason, "payload_bytes": e.payload_bytes,
            })
        })
        .collect();
    let metrics: Vec<Value> = reader
        .recent_metrics(limit)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|m| {
            json!({
                "at": m.ts, "kind": m.kind, "session_id": m.session_id, "tokens": m.tokens,
                "items": m.items, "basis": if m.estimated { "estimated" } else { "measured" },
            })
        })
        .collect();
    let admin = MemoryAdmin::new(&path);
    let stats = admin.stats(None).map_err(|e| e.to_string())?;
    let enabled = admin.is_enabled(None).map_err(|e| e.to_string())?;
    Ok(json!({
        "database": {"path": path.display().to_string(), "exists": true,
                     "total_events": reader.event_count().map_err(|e| e.to_string())?},
        "recent_events": events,
        "memory": {"enabled": enabled, "total": stats.total, "live": stats.live,
                   "superseded": stats.superseded, "user_sourced": stats.user_sourced},
        "token_metrics": metrics,
        "token_metrics_note": "basis=estimated is Kepler's heuristic, basis=measured was reported by the harness; they are never summed",
    }))
}

/// Same report as `kepler doctor --json`, probed with a short timeout and no autostart.
pub fn doctor_value() -> Result<Value, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let db = kepler_storage::dirs::db_path().map_err(|e| e.to_string());
    let report = rt.block_on(kepler_core::doctor::run(
        &Endpoint::from_env(),
        db.as_deref().map_err(Clone::clone),
    ));
    let mut v = report.to_json();
    if let Some(o) = v.as_object_mut() {
        o.insert("forge_model".into(), crate::forge_model::doctor_value());
    }
    Ok(v)
}
