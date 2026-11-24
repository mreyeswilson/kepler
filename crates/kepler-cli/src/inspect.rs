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
        "token_summary": token_summary(&reader)?,
        "token_metrics_note": "basis=estimated is Kepler's heuristic, basis=measured was reported by the harness; they are never summed",
    }))
}

/// What the Claude Code adapter does in the flow, and what it cannot (ADR-0027).
pub fn flow_doctor_value() -> Value {
    let cfg = kepler_policy::TokenGuardConfig::from_env();
    let off = |k: &str| crate::hook_flow::env_off(k);
    json!({
        "guardian_tokens": {
            "enabled": cfg.enabled,
            "read_max_lines": cfg.read_max_lines,
            "output_warn_tokens": cfg.output_warn_tokens,
            "does": [
                "PreToolUse Read: adds `limit` to an unbounded Read of a file longer than read_max_lines (never when offset/limit are already set)",
                "PostToolUse: one-line notes for repeated unchanged reads and very large results",
                "token_metrics: estimated savings/notes plus measured usage read from the transcript",
            ],
            "cannot": [
                "rewrite or compress the output of Bash/Grep/Glob or other built-in tools before the model sees it (not done: Kepler does not use `updatedToolOutput`)",
                "count tokens exactly: figures marked estimated are a heuristic; only transcript `usage` is measured",
                "guarantee savings: a bounded Read may cost an extra call if the model needs the rest",
            ],
            "disable": "KEPLER_GUARDIAN_TOKENS=off",
        },
        "sentinel_in_flow": {
            "enabled": !off("KEPLER_SENTINEL"),
            "audits": "git diff of the files the agent changed, at Stop (budget KEPLER_SENTINEL_BUDGET_MS, default 3000)",
            "project_check_configured": std::env::var("KEPLER_SENTINEL_CHECK").is_ok_and(|v| !v.trim().is_empty()),
            "cannot": ["inject context at Stop: findings go to the user (systemMessage) and to the model at the next prompt"],
        },
        "memory_in_flow": {
            "enabled": !off("KEPLER_MEMORY"),
            "does": "project-scoped brief at SessionStart, retrieval at each prompt, capture of explicit user phrases and observed facts",
            "cannot": ["store chats or tool output; capture inferences as user statements"],
        },
    })
}

pub fn flow_doctor_text() -> String {
    let v = flow_doctor_value();
    let g = &v["guardian_tokens"];
    let onoff = |b: &Value| if b.as_bool() == Some(true) { "on" } else { "off" };
    format!(
        "\nflow (Claude Code hooks, ADR-0027)\n  guardian tokens : {} (Read bounded above {} lines; notes above ~{} output tokens; KEPLER_GUARDIAN_TOKENS=off disables)\n  sentinel        : {} (audits changed files at Stop; optional KEPLER_SENTINEL_CHECK {})\n  memory          : {} (project brief at SessionStart)\n  not possible    : rewriting/compressing built-in tool output (Bash, Grep...) is not done; token figures are estimates except `usage_*` read from the transcript\n",
        onoff(&g["enabled"]),
        g["read_max_lines"],
        g["output_warn_tokens"],
        onoff(&v["sentinel_in_flow"]["enabled"]),
        if v["sentinel_in_flow"]["project_check_configured"] == true { "set" } else { "unset" },
        onoff(&v["memory_in_flow"]["enabled"]),
    )
}

fn basis(estimated: bool) -> &'static str {
    if estimated {
        "estimated"
    } else {
        "measured"
    }
}

/// Totals per kind, per project and per recent session, always split by basis.
/// `usage_*` kinds are measured (Claude Code transcript `usage`); `guardian_*` and
/// `memory_*` are Kepler estimates. Savings are estimates of tokens *not sent*, not measurements.
fn token_summary(reader: &Reader) -> Result<Value, String> {
    let by_kind: Vec<Value> = reader
        .metrics_summary(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|m| {
            json!({"kind": m.kind, "basis": basis(m.estimated), "tokens": m.tokens,
                   "items": m.items, "rows": m.rows})
        })
        .collect();
    let groups = reader.metrics_by_session(200).map_err(|e| e.to_string())?;
    let mut projects: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    let mut sessions: Vec<(String, String, Vec<Value>)> = Vec::new();
    for g in &groups {
        let row = json!({"kind": g.kind, "basis": basis(g.estimated), "tokens": g.tokens, "items": g.items});
        projects.entry(g.project_id.clone()).or_default().push(row.clone());
        let pos = sessions
            .iter()
            .position(|(p, s, _)| *p == g.project_id && *s == g.session_id);
        match pos {
            Some(i) => sessions[i].2.push(row),
            None if sessions.len() < 20 => {
                sessions.push((g.project_id.clone(), g.session_id.clone(), vec![row]));
            }
            None => {}
        }
    }
    Ok(json!({
        "by_kind": by_kind,
        "by_project": projects.into_iter()
            .map(|(p, rows)| json!({"project_id": p, "metrics": rows})).collect::<Vec<_>>(),
        "by_session": sessions.into_iter()
            .map(|(p, s, rows)| json!({"project_id": p, "session_id": s, "metrics": rows}))
            .collect::<Vec<_>>(),
        "scope": "last 200 metric groups for by_project/by_session; by_kind covers the retained rows (newest 5000)",
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
        o.insert("flow".into(), flow_doctor_value());
    }
    Ok(v)
}
