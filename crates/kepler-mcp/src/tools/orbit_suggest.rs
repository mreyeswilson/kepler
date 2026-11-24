//! `kepler_orbit_suggest_plan`: turns subtasks chosen by the model into an Orbit plan. Pure and
//! deterministic: no model call, no I/O besides asking the backend to validate the result.

use crate::tool::{integer, obj, string};
use crate::Backend;
use serde_json::{json, Map, Value};

const DEFAULT_PERMISSION_MODE: &str = "acceptEdits";
const INTEGRATE_ID: &str = "integrate";

pub fn schema() -> Value {
    obj(
        json!({
            "objective": string("What the whole job achieves (used as the plan name and in the summary)", 500),
            "subtasks": {
                "type": "array", "minItems": 1, "maxItems": 32,
                "description": "The parts YOU decided to delegate. Each needs `prompt` (sub-agent) or `command` (argv), never both.",
                "items": {
                    "type": "object", "required": ["id"], "additionalProperties": false,
                    "properties": {
                        "id": string("Short id: letters, digits, . _ - (max 64), e.g. api, ui, tests", 64),
                        "title": {"type": "string", "maxLength": 200, "description": "One line shown to the user"},
                        "prompt": string("Self-contained instructions for the sub-agent: goal, files/areas to touch and avoid, how to verify. It sees nothing of this conversation.", 8192),
                        "command": {"type": "array", "maxItems": 32, "items": {"type": "string", "minLength": 1, "maxLength": 1024},
                            "description": "argv instead of a prompt, e.g. [\"cargo\",\"test\"]"},
                        "depends_on": {"type": "array", "maxItems": 16, "items": {"type": "string", "minLength": 1, "maxLength": 64},
                            "description": "Ids of subtasks that must finish first; their branches are merged into this task's worktree"},
                        "agent_args": {"type": "array", "maxItems": 16, "items": {"type": "string", "minLength": 1, "maxLength": 256},
                            "description": "Overrides the default claude arguments for this agent task"},
                        "timeout_secs": integer("Per-attempt limit (default 600)", 1, 3600),
                        "max_retries": integer("Retries after failure (default 0)", 0, 3),
                    }
                }
            },
            "integration_prompt": string("Optional final agent task that depends on every subtask nobody else depends on: merge, resolve conflicts, run the tests. Id: `integrate`.", 8192),
            "max_parallel": integer("Tasks at once (1-4, default 2)", 1, 4),
            "permission_mode": {"type": "string", "enum": ["acceptEdits", "default", "plan"],
                "description": "Claude permission mode for agent tasks without their own agent_args (default acceptEdits so sub-agents can edit files in their worktree; never bypassPermissions)"},
        }),
        &["objective", "subtasks"],
    )
}

fn printable(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

fn strings(v: Option<&Value>) -> Vec<Value> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter(|x| x.is_string()).cloned().collect())
        .unwrap_or_default()
}

/// Build the plan JSON (not yet validated by Orbit).
pub fn build(a: &Map<String, Value>) -> Result<Value, String> {
    let objective = a.get("objective").and_then(Value::as_str).unwrap_or("");
    let subtasks = a
        .get("subtasks")
        .and_then(Value::as_array)
        .ok_or("`subtasks` is required")?;
    let mode = a
        .get("permission_mode")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PERMISSION_MODE);
    let mut tasks: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for (i, s) in subtasks.iter().enumerate() {
        let id = s.get("id").and_then(Value::as_str).unwrap_or("");
        let prompt = s.get("prompt").and_then(Value::as_str);
        let command = strings(s.get("command"));
        match (prompt.is_some(), !command.is_empty()) {
            (true, false) | (false, true) => {}
            _ => {
                return Err(format!(
                    "subtasks[{i}] (`{id}`): give exactly one of `prompt` or `command`"
                ))
            }
        }
        if ids.iter().any(|x| x == id) {
            return Err(format!("duplicate subtask id `{id}`"));
        }
        ids.push(id.to_string());
        let mut t = Map::new();
        t.insert("id".into(), json!(id));
        let title = s.get("title").and_then(Value::as_str).unwrap_or("");
        if !title.is_empty() {
            t.insert("description".into(), json!(printable(title, 200)));
        }
        if let Some(p) = prompt {
            t.insert("agent".into(), json!(p));
            let args = strings(s.get("agent_args"));
            let args = if args.is_empty() {
                vec![json!("--permission-mode"), json!(mode)]
            } else {
                args
            };
            t.insert("agent_args".into(), Value::Array(args));
        } else {
            t.insert("command".into(), Value::Array(command));
        }
        let deps = strings(s.get("depends_on"));
        if !deps.is_empty() {
            t.insert("depends_on".into(), Value::Array(deps));
        }
        for k in ["timeout_secs", "max_retries"] {
            if let Some(v) = s.get(k).filter(|v| v.is_u64()) {
                t.insert(k.into(), v.clone());
            }
        }
        tasks.push(Value::Object(t));
    }
    if let Some(ip) = a.get("integration_prompt").and_then(Value::as_str) {
        if ids.iter().any(|x| x == INTEGRATE_ID) {
            return Err(format!(
                "a subtask already uses the id `{INTEGRATE_ID}` reserved for the integration step"
            ));
        }
        // Leaves = subtasks nobody depends on.
        let depended: Vec<&str> = tasks
            .iter()
            .flat_map(|t| {
                t.get("depends_on")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
            })
            .collect();
        let leaves: Vec<Value> = ids
            .iter()
            .filter(|i| !depended.contains(&i.as_str()))
            .map(|i| json!(i))
            .collect();
        tasks.push(json!({
            "id": INTEGRATE_ID,
            "description": "Integrate the results of the previous tasks",
            "agent": ip,
            "agent_args": ["--permission-mode", mode],
            "depends_on": leaves,
        }));
    }
    let mut plan = json!({"name": printable(objective, 80), "tasks": tasks});
    if let Some(p) = a.get("max_parallel").filter(|v| v.is_u64()) {
        plan["max_parallel"] = p.clone();
    }
    Ok(plan)
}

pub fn handle(b: &dyn Backend, a: &Map<String, Value>) -> Result<String, String> {
    let plan = build(a)?;
    let mut args = Map::new();
    args.insert("plan".into(), plan.clone());
    let validation = b.op("kepler_orbit_plan", &args)?;
    let validation: Value = serde_json::from_str(&validation).unwrap_or(Value::String(validation));
    Ok(json!({
        "plan": plan,
        "validation": validation,
        "next": "Show the user this plan (tasks, dependencies, which run in parallel, which need approval, that agent tasks are real `claude -p` runs). Only after the user agrees, call kepler_orbit_run with this exact `plan`, then poll kepler_orbit_status.",
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn divide_and_delegate_builds_two_agents_and_an_integration_task() {
        let p = build(&args(json!({
            "objective": "Add login",
            "subtasks": [
                {"id": "api", "prompt": "write the API", "title": "API"},
                {"id": "ui", "prompt": "write the UI"},
            ],
            "integration_prompt": "merge and run tests",
            "max_parallel": 2,
        })))
        .unwrap();
        assert_eq!(p["name"], "Add login");
        assert_eq!(p["max_parallel"], 2);
        let t = p["tasks"].as_array().unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[0]["agent"], "write the API");
        assert_eq!(
            t[0]["agent_args"],
            json!(["--permission-mode", "acceptEdits"])
        );
        assert!(t[0].get("command").is_none());
        assert_eq!(t[2]["id"], "integrate");
        assert_eq!(t[2]["depends_on"], json!(["api", "ui"]));
    }

    #[test]
    fn integration_depends_only_on_leaves_and_commands_are_argv() {
        let p = build(&args(json!({
            "objective": "o\nbj",
            "subtasks": [
                {"id": "a", "prompt": "x"},
                {"id": "b", "command": ["cargo", "test"], "depends_on": ["a"], "timeout_secs": 90},
            ],
            "integration_prompt": "i",
            "permission_mode": "plan",
        })))
        .unwrap();
        assert_eq!(p["name"], "o bj");
        let t = p["tasks"].as_array().unwrap();
        assert_eq!(t[1]["command"], json!(["cargo", "test"]));
        assert_eq!(t[1]["timeout_secs"], 90);
        assert_eq!(t[2]["depends_on"], json!(["b"]));
        assert_eq!(t[2]["agent_args"], json!(["--permission-mode", "plan"]));
    }

    #[test]
    fn rejects_ambiguous_or_duplicate_subtasks() {
        let e = |v: Value| build(&args(v)).unwrap_err();
        assert!(e(json!({"objective":"o","subtasks":[{"id":"a"}]})).contains("exactly one"));
        assert!(
            e(json!({"objective":"o","subtasks":[{"id":"a","prompt":"p","command":["x"]}]}))
                .contains("exactly one")
        );
        assert!(e(
            json!({"objective":"o","subtasks":[{"id":"a","prompt":"p"},{"id":"a","prompt":"q"}]})
        )
        .contains("duplicate"));
        assert!(e(json!({"objective":"o","subtasks":[{"id":"integrate","prompt":"p"}],"integration_prompt":"i"}))
            .contains("reserved"));
    }
}
