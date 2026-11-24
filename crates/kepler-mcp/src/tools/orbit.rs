//! Orbit tools: validate, start, supervise and finish a DAG of tasks in isolated git worktrees.
//! `agent` tasks run `claude -p` (argv, never a shell) in their own worktree: that is how a
//! session delegates work to sub-agents. Nothing here starts agents by itself; the model proposes,
//! the user approves, Orbit supervises.

use crate::tool::{boolean, flag, integer, obj, string, Hints, Tool};
use crate::Backend;
use serde_json::{json, Map, Value};

pub const MAX_PLAN_TASKS: i64 = 64;

/// JSON Schema of an Orbit plan (mirrors `kepler_orbit::PlanFile`, which is `deny_unknown_fields`).
pub fn plan_schema() -> Value {
    json!({
        "type": "object",
        "description": "Orbit plan: a DAG of tasks. Each task has EITHER `agent` (a prompt for a sub-agent run as `claude -p` in the task's own worktree) OR `command` (argv list, never a shell string).",
        "required": ["name", "tasks"],
        "additionalProperties": false,
        "properties": {
            "name": string("Short plan name (max 80 characters)", 80),
            "max_parallel": integer("Tasks running at once (1-4, default 2)", 1, 4),
            "tasks": {
                "type": "array", "minItems": 1, "maxItems": MAX_PLAN_TASKS,
                "items": {
                    "type": "object", "required": ["id"], "additionalProperties": false,
                    "properties": {
                        "id": string("Task id: letters, digits, . _ - (max 64)", 64),
                        "description": {"type": "string", "maxLength": 500},
                        "agent": string("Prompt for a sub-agent (`claude -p <prompt>` in this task's worktree). Make it self-contained: the sub-agent sees none of this conversation.", 8192),
                        "agent_args": {"type": "array", "maxItems": 16, "items": {"type": "string", "minLength": 1, "maxLength": 256},
                            "description": "Extra claude arguments, e.g. [\"--permission-mode\",\"acceptEdits\"]"},
                        "command": {"type": "array", "maxItems": 32, "items": {"type": "string", "minLength": 1, "maxLength": 1024},
                            "description": "argv of a plain command, e.g. [\"cargo\",\"test\"]"},
                        "depends_on": {"type": "array", "maxItems": 16, "items": {"type": "string", "minLength": 1, "maxLength": 64}},
                        "timeout_secs": integer("Per-attempt limit in seconds (default 600)", 1, 3600),
                        "max_retries": integer("Retries after failure (default 0)", 0, 3),
                        "require_approval": boolean("Force an approval checkpoint before this task runs"),
                    }
                }
            }
        }
    })
}

fn run_ref(extra: Value, required: &[&str]) -> Value {
    let mut props =
        json!({"run": string("Run id returned by kepler_orbit_run (looks like orb-...)", 64)});
    if let (Some(p), Some(e)) = (props.as_object_mut(), extra.as_object()) {
        p.extend(e.clone());
    }
    let mut req = vec!["run"];
    req.extend(required);
    obj(props, &req)
}

fn task_prop() -> Value {
    json!({"task": string("Task id inside the run", 64)})
}

fn approve(b: &dyn Backend, a: &Map<String, Value>) -> Result<String, String> {
    if !flag(a, "confirm") {
        return Err("not approved: `confirm: true` is required, and it may only be set after the user explicitly approved THIS task in the chat. Call kepler_orbit_status, show the user the task's command and approval_reason, ask, and retry with confirm: true only on a clear yes.".into());
    }
    b.op("kepler_orbit_approve", a)
}

fn cleanup(b: &dyn Backend, a: &Map<String, Value>) -> Result<String, String> {
    if !flag(a, "confirm") {
        return Err("nothing removed: `confirm: true` is required. Cleanup deletes the run's worktrees, task branches (the work they hold) and logs; merge what you need first and ask the user before confirming.".into());
    }
    b.op("kepler_orbit_cleanup", a)
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "kepler_orbit_suggest_plan",
            title: "Build an Orbit plan from subtasks",
            description: "Use when a task is large and splits into independent parts that can be delegated to sub-agents (e.g. frontend + backend + tests, then an integration step). Pass the objective and the subtasks YOU decided on (each a self-contained prompt, or a command argv, plus depends_on). Builds the structured plan JSON deterministically (no model is called, nothing runs) and validates it. Do not use it for small tasks: do those yourself. Next: show the plan to the user, and only after they agree call kepler_orbit_run.",
            schema: super::orbit_suggest::schema,
            hints: Hints::READ,
            handler: Some(super::orbit_suggest::handle),
        },
        Tool {
            name: "kepler_orbit_plan",
            title: "Validate an Orbit plan (dry run)",
            description: "Validate a plan WITHOUT running anything: cycles, limits, secret-shaped values and Guardian verdicts (a denied command invalidates the plan; a destructive one becomes an approval checkpoint). Returns the DAG: execution order, parallel waves and which tasks will need approval. Use before kepler_orbit_run or to check a hand-written plan.",
            schema: || obj(json!({"plan": plan_schema()}), &["plan"]),
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_orbit_run",
            title: "Start an Orbit run",
            description: "Start executing a validated plan: creates a run (one git worktree and branch per task, outside the repository) and returns a run_id IMMEDIATELY without waiting; tasks run in the background under Orbit's supervision. Agent tasks launch real `claude -p` sub-agents that edit files and use the user's quota. Ask the user to approve the plan first (show kepler_orbit_plan output) and never start agents just because they are installed. Then poll kepler_orbit_status. The repository's working tree is not modified: results land on branches kepler/orbit/<run>/<task>.",
            schema: || {
                obj(
                    json!({
                        "plan": plan_schema(),
                        "repo": string("Path of the git repository the tasks work on (absolute is safest)", 1024),
                        "base": string("Revision every task starts from (default HEAD)", 128),
                        "project": string("Project id for the records (default: derived from repo)", 256),
                        "max_parallel": integer("Override the plan's max_parallel (1-4)", 1, 4),
                    }),
                    &["plan", "repo"],
                )
            },
            hints: Hints::WRITE.open_world(),
            handler: None,
        },
        Tool {
            name: "kepler_orbit_list",
            title: "List recent Orbit runs",
            description: "List recent Orbit runs (id, status, name). Use when you lost a run_id or the user asks what is running.",
            schema: || obj(json!({"limit": integer("How many runs (default 10)", 1, 50)}), &[]),
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_orbit_status",
            title: "Orbit run status",
            description: "Status of a run: each task's state (pending, running, awaiting_approval, done, failed, skipped...), attempts, exit code, branch, changed files, approval reason, plus warnings when independent tasks changed the same file. Poll this after kepler_orbit_run (do not busy-loop: check, then work or wait for the user). When status is needs_approval, show the user the waiting task; when done, summarize per task and tell the user which branches to review and merge.",
            schema: || run_ref(json!({}), &[]),
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_orbit_logs",
            title: "Orbit task output",
            description: "Captured output of one task (stdout and stderr, secrets redacted, truncated to the last part). Use to see why a task failed or what a sub-agent reported.",
            schema: || {
                run_ref(
                    json!({
                        "task": string("Task id inside the run", 64),
                        "stream": {"type": "string", "enum": ["both", "stdout", "stderr"], "description": "Default both"},
                        "tail_bytes": integer("How many trailing bytes to return (default 8192, max 24576)", 256, 24576),
                    }),
                    &["task"],
                )
            },
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_orbit_approve",
            title: "Approve a waiting Orbit task",
            description: "Grant the approval checkpoint of a task in status awaiting_approval (it holds a destructive command or the plan required approval). ONLY use after the user explicitly approved that specific task in the chat; show them the command and reason first. Requires confirm: true. One-shot. It does not start anything: call kepler_orbit_resume afterwards.",
            schema: || run_ref(json!({"task": task_prop()["task"].clone(), "confirm": boolean("Must be true; set only after the user's explicit approval in the chat")}), &["task", "confirm"]),
            hints: Hints::WRITE.idempotent(),
            handler: Some(approve),
        },
        Tool {
            name: "kepler_orbit_deny",
            title: "Deny a waiting Orbit task",
            description: "Deny a task in status awaiting_approval: it fails and everything depending on it is skipped. Use when the user refuses it or it is no longer wanted. Safe: it only prevents work.",
            schema: || run_ref(task_prop(), &["task"]),
            hints: Hints::WRITE.idempotent().non_destructive(),
            handler: None,
        },
        Tool {
            name: "kepler_orbit_resume",
            title: "Resume an Orbit run",
            description: "Continue a run in the background after approvals/denials (or after a crash): starts every task that may now run. Returns immediately; poll kepler_orbit_status. Fails if the run is already running.",
            schema: || run_ref(json!({}), &[]),
            hints: Hints::WRITE.open_world(),
            handler: None,
        },
        Tool {
            name: "kepler_orbit_cancel",
            title: "Cancel a running Orbit run",
            description: "Stop a running run: running tasks (and their process trees) are killed and pending ones cancelled. Use when the user asks to stop or the plan is wrong. Work already committed on task branches is kept.",
            schema: || run_ref(json!({}), &[]),
            hints: Hints::WRITE.idempotent(),
            handler: None,
        },
        Tool {
            name: "kepler_orbit_cleanup",
            title: "Delete an Orbit run's worktrees and branches",
            description: "DELETES the run's worktrees, task branches (the work they hold) and logs. Only after the results were merged or the user said to discard them; ask first. Requires confirm: true. Refuses while the run is running.",
            schema: || {
                run_ref(
                    json!({
                        "confirm": boolean("Must be true; set only after the user agreed"),
                        "delete_records": boolean("Also delete the run's database records"),
                    }),
                    &["confirm"],
                )
            },
            hints: Hints::WRITE.idempotent(),
            handler: Some(cleanup),
        },
    ]
}
