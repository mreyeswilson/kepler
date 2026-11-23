//! `kepler orbit ...`: run a DAG of command tasks, each in its own git worktree, with approval
//! checkpoints for destructive commands. Works on the local database directly (no daemon).

use clap::Subcommand;
use kepler_orbit::{parse_plan, validate, Orbit, RunSummary, ValidPlan, MAX_PARALLEL};
use kepler_policy::Guardian;
use kepler_storage::Writer;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum OrbitCmd {
    /// Validate a plan (cycles, limits, secrets, Guardian verdicts) and print the order. Runs nothing.
    Plan {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Create a run from a plan and execute everything that may run now.
    Run {
        file: PathBuf,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Revision every task starts from.
        #[arg(long, default_value = "HEAD")]
        base: String,
        #[arg(long)]
        project: Option<String>,
        /// Override the plan's max_parallel (1 to 4).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4))]
        max_parallel: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// List recent runs.
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show a run, its tasks, approvals and overlapping changes.
    Status {
        run: String,
        #[arg(long)]
        json: bool,
    },
    /// Approve a task that is waiting (a destructive command). Then `resume`.
    Approve { run: String, task: String },
    /// Deny a waiting task: it fails and its dependents are skipped.
    Deny { run: String, task: String },
    /// Continue a run after approvals.
    Resume {
        run: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop a running run (from another terminal): tasks are killed, pending ones cancelled.
    Cancel { run: String },
    /// Remove worktrees, branches and logs of a run. Task branches hold the work: merge what you
    /// need first. Needs --yes.
    Cleanup {
        run: String,
        /// Also delete the run from the database.
        #[arg(long)]
        delete_records: bool,
        #[arg(long)]
        yes: bool,
    },
}

fn orbit() -> Result<Orbit, String> {
    let db = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
    let root = kepler_storage::dirs::data_dir()
        .map_err(|e| e.to_string())?
        .join("orbit");
    let writer = Writer::start(&db, Default::default()).map_err(|e| e.to_string())?;
    let (rules, warnings) = kepler_core::pipeline::load_guardian_rules();
    for w in warnings {
        eprintln!("kepler: warning: {w}");
    }
    Ok(Orbit::new(Arc::new(writer), root).with_guardian(Guardian::new(rules)))
}

fn load_plan(file: &PathBuf) -> Result<ValidPlan, String> {
    let meta = std::fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?;
    if meta.len() > 256 * 1024 {
        return Err("plan file is larger than 256 KiB".into());
    }
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let (rules, _) = kepler_core::pipeline::load_guardian_rules();
    let plan = parse_plan(&text).map_err(|e| e.to_string())?;
    validate(&plan, &Guardian::new(rules)).map_err(|e| e.to_string())
}

fn summary_json(s: &RunSummary) -> Value {
    json!({
        "run": s.run.run_id, "name": s.run.name, "status": s.run.status,
        "base": s.run.base_ref, "repo": s.run.repo_path, "max_parallel": s.run.max_parallel,
        "tasks": s.tasks.iter().map(|t| json!({
            "id": t.key, "status": t.status, "attempts": t.attempts, "exit_code": t.exit_code,
            "error": t.error, "branch": t.branch, "worktree": t.worktree,
            "changed_files": t.changed_files, "approval": t.approval,
            "approval_reason": t.approval_reason, "depends_on": t.deps,
        })).collect::<Vec<_>>(),
        "overlaps": s.overlaps.iter().map(|o| json!({"a": o.a, "b": o.b, "files": o.files})).collect::<Vec<_>>(),
    })
}

fn print_summary(s: &RunSummary) {
    println!(
        "run {} \"{}\": {} (max_parallel {})",
        s.run.run_id, s.run.name, s.run.status, s.run.max_parallel
    );
    for t in &s.tasks {
        let mut line = format!("  {:20} {:18} attempts={}", t.key, t.status, t.attempts);
        if let Some(c) = t.exit_code {
            line.push_str(&format!(" exit={c}"));
        }
        if !t.changed_files.is_empty() {
            line.push_str(&format!(" changed={}", t.changed_files.len()));
        }
        println!("{line}");
        if t.status == "awaiting_approval" {
            println!(
                "      needs approval ({}): kepler orbit approve {} {}",
                t.approval_reason, s.run.run_id, t.key
            );
        }
        if let Some(e) = &t.error {
            println!("      {e}");
        }
        if let Some(b) = &t.branch {
            println!("      branch {b}");
        }
    }
    for o in &s.overlaps {
        println!(
            "  WARNING: independent tasks {} and {} changed the same file(s): {}",
            o.a,
            o.b,
            o.files.join(", ")
        );
    }
}

fn finish(s: &RunSummary, json: bool) -> ExitCode {
    if json {
        println!("{}", summary_json(s));
    } else {
        print_summary(s);
    }
    match s.run.status.as_str() {
        "done" => ExitCode::SUCCESS,
        "needs_approval" => ExitCode::from(3),
        _ => ExitCode::FAILURE,
    }
}

pub fn run(cmd: OrbitCmd) -> ExitCode {
    match exec(cmd) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("kepler: {e}");
            ExitCode::from(2)
        }
    }
}

fn exec(cmd: OrbitCmd) -> Result<ExitCode, String> {
    match cmd {
        OrbitCmd::Plan { file, json } => {
            let plan = load_plan(&file)?;
            let order = plan.order();
            if json {
                println!(
                    "{}",
                    json!({
                        "name": plan.name, "max_parallel": plan.max_parallel, "order": order,
                        "needs_approval": plan.tasks.iter().filter(|t| t.needs_approval).map(|t| json!({"id": t.key, "reason": t.approval_reason})).collect::<Vec<_>>(),
                    })
                );
            } else {
                println!(
                    "plan \"{}\": {} tasks, max_parallel {} (limit {MAX_PARALLEL})",
                    plan.name,
                    plan.tasks.len(),
                    plan.max_parallel
                );
                println!("order: {}", order.join(" -> "));
                for t in plan.tasks.iter().filter(|t| t.needs_approval) {
                    println!("  needs approval: {} ({})", t.key, t.approval_reason);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        OrbitCmd::Run {
            file,
            repo,
            base,
            project,
            max_parallel,
            json,
        } => {
            let mut plan = load_plan(&file)?;
            if let Some(p) = max_parallel {
                plan.max_parallel = p;
            }
            let o = orbit()?;
            let project = project.unwrap_or_else(|| crate::sentinel_cmd::project_for(&repo));
            let id = o
                .create_run(&repo, &base, &project, &plan)
                .map_err(|e| e.to_string())?;
            eprintln!("kepler: created run {id}");
            let s = o.execute(&id).map_err(|e| e.to_string())?;
            Ok(finish(&s, json))
        }
        OrbitCmd::Resume { run, json } => {
            let o = orbit()?;
            let s = o.execute(&run).map_err(|e| e.to_string())?;
            Ok(finish(&s, json))
        }
        OrbitCmd::List { limit } => {
            let path = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
            if !path.exists() {
                println!("no runs");
                return Ok(ExitCode::SUCCESS);
            }
            let writer = Writer::start(&path, Default::default()).map_err(|e| e.to_string())?;
            let rows = writer
                .exec(std::time::Duration::from_secs(5), move |c| {
                    kepler_storage::list_runs(c, limit)
                })
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                println!("no runs");
            }
            for r in rows {
                println!(
                    "{} {:15} {} \"{}\"",
                    r.run_id, r.status, r.created_at, r.name
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        OrbitCmd::Status { run, json } => {
            let o = orbit()?;
            let s = o.status(&run).map_err(|e| e.to_string())?;
            if json {
                println!("{}", summary_json(&s));
            } else {
                print_summary(&s);
            }
            Ok(ExitCode::SUCCESS)
        }
        OrbitCmd::Approve { run, task } => resolve(&run, &task, true),
        OrbitCmd::Deny { run, task } => resolve(&run, &task, false),
        OrbitCmd::Cancel { run } => {
            orbit()?.request_cancel(&run).map_err(|e| e.to_string())?;
            println!("cancel requested; the executor stops its tasks within a moment");
            Ok(ExitCode::SUCCESS)
        }
        OrbitCmd::Cleanup {
            run,
            delete_records,
            yes,
        } => {
            if !yes {
                return Err("cleanup deletes worktrees and task branches (the work they hold); pass --yes to confirm".into());
            }
            let n = orbit()?
                .cleanup(&run, delete_records)
                .map_err(|e| e.to_string())?;
            println!("removed {n} worktree(s)/branch(es)");
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn resolve(run: &str, task: &str, grant: bool) -> Result<ExitCode, String> {
    let done = orbit()?
        .resolve_approval(run, task, grant)
        .map_err(|e| e.to_string())?;
    if done {
        println!(
            "{} {task}; `kepler orbit resume {run}` continues the run",
            if grant { "approved" } else { "denied" }
        );
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!("task {task} is not waiting for approval"))
    }
}
