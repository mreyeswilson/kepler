//! JSON API. Reads open a short-lived read-only SQLite connection per request; nothing here
//! returns prompts or tool output (Kepler stores none). Memory text is redacted again on the way
//! out.

use crate::http::{Request, Response};
use crate::UiConfig;
use kepler_memory::{row_to_json, MemoryAdmin};
use kepler_storage::{FindingFilter, MemoryFilter, Reader, Severity, Writer};
use serde_json::{json, Value};
use std::time::Duration;

fn limit_param(req: &Request, default: u32, max: u32) -> u32 {
    req.query
        .get("limit")
        .and_then(|v| v.parse::<u32>().ok())
        .map_or(default, |n| n.clamp(1, max))
}

fn err(status: u16, msg: &str) -> Response {
    Response::json(status, &json!({ "error": msg }))
}

pub async fn get(cfg: &UiConfig, path: &str, req: &Request) -> Response {
    let cfg = cfg.clone();
    let path = path.to_string();
    let limit = limit_param(req, 50, 500);
    let min_sev = req.query.get("min_severity").cloned();
    let query = req.query.clone();
    // The daemon probe is the only async part (and the only one that talks to another process).
    let daemon = if path == "/api/summary" {
        Some(probe_daemon().await)
    } else {
        None
    };
    let result = tokio::task::spawn_blocking(move || {
        match crate::views::route(&cfg, &path, &query, daemon) {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => route(&cfg, &path, limit, min_sev.as_deref()),
            Err(e) => Err(e),
        }
    })
    .await
    .unwrap_or_else(|_| Err("internal error".into()));
    match result {
        Ok(Some(v)) => Response::json(200, &v),
        Ok(None) => err(404, "unknown endpoint"),
        Err(e) => err(500, &e),
    }
}

/// Ask the daemon (if any) for its version with a short timeout. Never starts a daemon.
async fn probe_daemon() -> Value {
    let endpoint = kepler_ipc::Endpoint::from_env();
    let mut hello = kepler_ipc::HelloOptions::new("kepler-ui");
    hello.timeout = Duration::from_millis(500);
    let desc = endpoint.describe();
    match kepler_ipc::Client::connect(&endpoint, hello).await {
        Ok(mut c) => match c.ping().await {
            Ok(()) => {
                json!({"state": "running", "version": c.server_version.clone(), "endpoint": desc,
                "uptime_secs": null, "note": "el daemon no expone su uptime"})
            }
            Err(_) => json!({"state": "unresponsive", "endpoint": desc}),
        },
        Err(_) => json!({"state": "stopped", "endpoint": desc}),
    }
}

fn route(
    cfg: &UiConfig,
    path: &str,
    limit: u32,
    min_sev: Option<&str>,
) -> Result<Option<Value>, String> {
    let exists = cfg.db_path.exists();
    let empty = |note: &str| json!({ "database": {"exists": false}, "note": note });
    let reader = || Reader::open(&cfg.db_path).map_err(|e| e.to_string());
    let v = match path {
        "/api/overview" => {
            if !exists {
                return Ok(Some(overview_empty(cfg)));
            }
            let r = reader()?;
            let findings = r
                .findings(&FindingFilter {
                    project_id: None,
                    min_severity: None,
                    include_dismissed: false,
                    limit: 1000,
                })
                .map_err(|e| e.to_string())?;
            let mut by_sev = serde_json::Map::new();
            for f in &findings {
                let n = by_sev.entry(f.severity.clone()).or_insert(json!(0));
                *n = json!(n.as_u64().unwrap_or(0) + 1);
            }
            let runs = r.orbit_runs(200).map_err(|e| e.to_string())?;
            let mut awaiting = 0usize;
            for run in &runs {
                awaiting += r
                    .orbit_tasks(&run.run_id)
                    .map_err(|e| e.to_string())?
                    .iter()
                    .filter(|t| t.status == "awaiting_approval")
                    .count();
            }
            let admin = MemoryAdmin::new(&cfg.db_path);
            let stats = admin.stats(None).map_err(|e| e.to_string())?;
            json!({
                "version": env!("CARGO_PKG_VERSION"),
                "actions_enabled": cfg.allow_actions,
                "database": {"exists": true, "total_events": r.event_count().map_err(|e| e.to_string())?},
                "memory": {"total": stats.total, "live": stats.live, "superseded": stats.superseded,
                           "user_sourced": stats.user_sourced,
                           "enabled": admin.is_enabled(None).map_err(|e| e.to_string())?},
                "findings": {"open": findings.len(), "by_severity": by_sev},
                "orbit": {"runs": runs.len(), "tasks_awaiting_approval": awaiting},
            })
        }
        "/api/metrics" => {
            if !exists {
                return Ok(Some(empty("no database yet")));
            }
            let rows = reader()?.recent_metrics(limit).map_err(|e| e.to_string())?;
            json!({ "metrics": rows.into_iter().map(|m| json!({
                "at": m.ts, "kind": m.kind, "session_id": m.session_id, "tokens": m.tokens,
                "items": m.items, "basis": if m.estimated { "estimated" } else { "measured" },
            })).collect::<Vec<_>>(),
            "note": "estimated and measured token figures are never summed" })
        }
        "/api/memories" => {
            let rows = MemoryAdmin::new(&cfg.db_path)
                .list(&MemoryFilter {
                    project_id: None,
                    scope: None,
                    include_superseded: false,
                    limit,
                })
                .map_err(|e| e.to_string())?;
            json!({ "memories": rows.iter().map(row_to_json).collect::<Vec<_>>() })
        }
        "/api/findings" => {
            if !exists {
                return Ok(Some(empty("no database yet")));
            }
            let min = match min_sev {
                Some(s) => Some(
                    Severity::parse(s)
                        .ok_or("min_severity must be info|low|medium|high|critical")?,
                ),
                None => None,
            };
            let rows = reader()?
                .findings(&FindingFilter {
                    project_id: None,
                    min_severity: min,
                    include_dismissed: false,
                    limit,
                })
                .map_err(|e| e.to_string())?;
            json!({ "findings": rows.into_iter().map(|r| json!({
                "id": r.id, "at": r.ts, "project": r.project_id, "source": r.source, "rule": r.rule_id,
                "severity": r.severity, "confidence": r.confidence, "path": r.path, "line": r.line,
                "message": r.message, "evidence": r.evidence, "status": r.status,
            })).collect::<Vec<_>>() })
        }
        "/api/orbit" => {
            if !exists {
                return Ok(Some(json!({ "runs": [] })));
            }
            let r = reader()?;
            let mut out = Vec::new();
            for run in r.orbit_runs(limit.min(50)).map_err(|e| e.to_string())? {
                let tasks = r.orbit_tasks(&run.run_id).map_err(|e| e.to_string())?;
                out.push(json!({
                    "run_id": run.run_id, "name": run.name, "status": run.status,
                    "max_parallel": run.max_parallel, "created_at": run.created_at,
                    "tasks": tasks.iter().map(|t| json!({
                        "id": t.key, "status": t.status, "attempts": t.attempts,
                        "exit_code": t.exit_code, "error": t.error, "branch": t.branch,
                        "changed_files": t.changed_files, "approval": t.approval,
                        "approval_reason": t.approval_reason, "depends_on": t.deps,
                    })).collect::<Vec<_>>(),
                }));
            }
            json!({ "runs": out })
        }
        _ => return Ok(None),
    };
    Ok(Some(v))
}

fn overview_empty(cfg: &UiConfig) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "actions_enabled": cfg.allow_actions,
        "database": {"exists": false},
        "note": "nothing recorded yet; the database is created when the daemon first runs",
    })
}

/// `POST /api/orbit/resolve` `{run, task, grant, confirm}`: approve or deny a waiting task.
pub async fn resolve(cfg: &UiConfig, req: &Request) -> Response {
    if !cfg.allow_actions {
        return err(
            403,
            "actions are disabled; start `kepler ui --allow-actions`",
        );
    }
    let ct = req
        .headers
        .get("content-type")
        .map(|c| c.to_ascii_lowercase())
        .unwrap_or_default();
    if !ct.starts_with("application/json") {
        return err(415, "content-type must be application/json");
    }
    let Ok(body) = serde_json::from_slice::<Value>(&req.body) else {
        return err(400, "invalid JSON");
    };
    let (Some(run), Some(task), Some(grant), Some(confirm)) = (
        body["run"].as_str(),
        body["task"].as_str(),
        body["grant"].as_bool(),
        body["confirm"].as_str(),
    ) else {
        return err(400, "expected {run, task, grant, confirm}");
    };
    if confirm != task || run.len() > 128 || task.len() > 128 {
        return err(400, "confirmation must equal the task id");
    }
    let (run, task) = (run.to_string(), task.to_string());
    let db = cfg.db_path.clone();
    let done = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        if !db.exists() {
            return Err("no database yet".into());
        }
        let writer = Writer::start(&db, Default::default()).map_err(|e| e.to_string())?;
        writer
            .exec(Duration::from_secs(5), move |c| {
                kepler_storage::resolve_approval(c, &run, &task, grant)
            })
            .map_err(|e| e.to_string())
    })
    .await
    .unwrap_or_else(|_| Err("internal error".into()));
    match done {
        Ok(true) => Response::json(
            200,
            &json!({ "ok": true, "note": "run `kepler orbit resume <run>` to continue it" }),
        ),
        Ok(false) => err(409, "that task is not waiting for approval"),
        Err(e) => err(500, &e),
    }
}
