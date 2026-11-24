//! Sentinel tools: deterministic change audit, stored findings and project checks.

use crate::tool::{boolean, integer, obj, string, Hints, Tool};
use crate::Backend;
use serde_json::{json, Map, Value};

fn audit(b: &dyn Backend, a: &Map<String, Value>) -> Result<String, String> {
    if a.get("staged") == Some(&Value::Bool(true)) && a.contains_key("base") {
        return Err("`staged` and `base` are mutually exclusive".into());
    }
    b.op("kepler_sentinel_audit", a)
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "kepler_sentinel_audit",
            title: "Audit uncommitted changes (Sentinel)",
            description: "Audit a repository's changes with Sentinel's deterministic rules (secrets, risky patterns, suspicious edits). Reads `git diff` only and never runs project code. Use before committing or after sub-agents finished, to look for problems in the diff. Defaults to the whole working tree vs HEAD; `staged` limits it to the index; `base` compares with another revision. Findings are not stored.",
            schema: || {
                obj(
                    json!({
                        "repo": string("Path of the git repository", 1024),
                        "staged": boolean("Only staged changes"),
                        "base": string("Compare the working tree with this revision instead of HEAD", 128),
                    }),
                    &["repo"],
                )
            },
            hints: Hints::READ,
            handler: Some(audit),
        },
        Tool {
            name: "kepler_sentinel_list",
            title: "List stored Sentinel findings",
            description: "List findings Kepler stored earlier (newest first), e.g. from kepler_sentinel_check or the CLI. Use to see what is already known before re-auditing.",
            schema: || {
                obj(
                    json!({
                        "project": string("Project id filter", 256),
                        "min_severity": {"type": "string", "enum": ["info", "low", "medium", "high", "critical"]},
                        "limit": integer("Max rows (default 30)", 1, 100),
                    }),
                    &[],
                )
            },
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_sentinel_check",
            title: "Run a project check (Sentinel)",
            description: "RUNS a project command (e.g. lint or tests) as argv under a time limit, Guardian-gated, and turns its failures into findings (which are stored). Use when the user wants the project's own checks run; for just reading a diff use kepler_sentinel_audit. Commands the Guardian says need approval are skipped, denied ones never run. The command is argv, never a shell string.",
            schema: || {
                obj(
                    json!({
                        "repo": string("Path of the git repository (the command's working directory)", 1024),
                        "name": string("Check name: letters, digits, - _", 40),
                        "argv": {"type": "array", "minItems": 1, "maxItems": 32,
                            "items": {"type": "string", "minLength": 1, "maxLength": 1024},
                            "description": "Program and arguments, e.g. [\"cargo\",\"test\",\"--quiet\"]"},
                        "timeout_secs": integer("Wall-clock limit (default 120, max 600)", 1, 600),
                    }),
                    &["repo", "name", "argv"],
                )
            },
            hints: Hints::WRITE.open_world(),
            handler: None,
        },
    ]
}
