//! Orbit (KPL-orbit / Delivery 5): a bounded DAG scheduler for command tasks with git worktree
//! isolation and approval checkpoints for destructive commands.
//!
//! * Plans are JSON with argv commands (never shell strings), validated for cycles, limits,
//!   secrets and Guardian verdicts (`deny` invalidates the plan, `require_approval` pauses the task).
//! * Each task runs in its own worktree and branch outside the repository; dependents start from the
//!   base commit with their dependencies' branches merged (a conflict fails the task, never the repo).
//! * Concurrency is bounded (1 to 4), every task has a timeout and is killed as a process tree.
//! * Orbit supervises child processes. The `agent` task preset runs `claude -p "<prompt>"` (or `KEPLER_AGENT_BIN`) as such a child; Orbit assumes no harness API.

mod plan;
mod runner;
mod worktree;

pub use plan::{
    parse_plan, validate, validate_with, PlanError, PlanFile, PlanTask, ValidPlan,
    DEFAULT_PARALLEL, MAX_PARALLEL,
};
pub use runner::{overlaps, Orbit, OrbitError, Overlap, RunSummary};
pub use worktree::{remove_worktree, resolve_base};
