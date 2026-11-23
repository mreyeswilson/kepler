//! Orbit persistence: `orbit_runs`, `tasks`, `task_dependencies`. Commands are stored as an argv
//! JSON array (the planner refuses secrets in them); outputs are never stored, only exit codes and
//! a short redacted error.

use crate::redact::redact;
use crate::StorageError;
use rusqlite::{params, Connection, OptionalExtension};

pub const MAX_TASKS_PER_RUN: usize = 64;
pub const MAX_RUNS: i64 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    pub run_id: String,
    pub project_id: String,
    pub repo_path: String,
    /// Resolved commit SHA the run is based on.
    pub base_ref: String,
    pub name: String,
    pub max_parallel: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub key: String,
    pub description: String,
    pub argv: Vec<String>,
    pub timeout_secs: u32,
    pub max_retries: u32,
    /// `true`: starts as `awaiting_approval` until `resolve_approval`.
    pub needs_approval: bool,
    pub approval_reason: String,
    pub deps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub run_id: String,
    pub project_id: String,
    pub repo_path: String,
    pub base_ref: String,
    pub name: String,
    pub status: String,
    pub max_parallel: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub key: String,
    pub description: String,
    pub argv: Vec<String>,
    pub status: String,
    pub attempts: u32,
    pub max_retries: u32,
    pub timeout_secs: u32,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub changed_files: Vec<String>,
    /// `none`, `required`, `granted` or `denied`.
    pub approval: String,
    pub approval_reason: String,
    pub deps: Vec<String>,
}

/// Partial update; `None` leaves the column alone.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub status: Option<&'static str>,
    pub attempts: Option<u32>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub changed_files: Option<Vec<String>>,
    pub mark_started: bool,
    pub mark_ended: bool,
}

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

pub fn create_run(conn: &Connection, run: &NewRun, tasks: &[NewTask]) -> Result<(), StorageError> {
    if tasks.len() > MAX_TASKS_PER_RUN {
        return Err(StorageError::Rejected("too many tasks in one run"));
    }
    conn.execute(
        "INSERT INTO orbit_runs (run_id, project_id, repo_path, base_ref, name, status, max_parallel)
         VALUES (?1, ?2, ?3, ?4, ?5, 'created', ?6)",
        params![
            run.run_id,
            run.project_id,
            run.repo_path,
            run.base_ref,
            run.name,
            run.max_parallel
        ],
    )?;
    for t in tasks {
        let argv = serde_json_array(&t.argv);
        let (status, approval) = if t.needs_approval {
            ("awaiting_approval", "required")
        } else {
            ("pending", "none")
        };
        conn.execute(
            "INSERT INTO tasks (run_id, task_key, description, command, status, max_retries,
                                timeout_secs, approval, approval_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run.run_id,
                t.key,
                t.description.chars().take(200).collect::<String>(),
                argv,
                status,
                t.max_retries,
                t.timeout_secs,
                approval,
                t.approval_reason.chars().take(200).collect::<String>()
            ],
        )?;
    }
    for t in tasks {
        for d in &t.deps {
            conn.execute(
                "INSERT INTO task_dependencies (task_id, depends_on)
                 SELECT a.id, b.id FROM tasks a, tasks b
                 WHERE a.run_id = ?1 AND a.task_key = ?2 AND b.run_id = ?1 AND b.task_key = ?3",
                params![run.run_id, t.key, d],
            )?;
        }
    }
    // Keep the newest runs only (tasks and dependencies go with them).
    let old: Vec<String> = {
        let mut s =
            conn.prepare("SELECT run_id FROM orbit_runs ORDER BY rowid DESC LIMIT -1 OFFSET ?1")?;
        let rows = s.query_map([MAX_RUNS], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for r in old {
        delete_run(conn, &r)?;
    }
    Ok(())
}

fn serde_json_array(items: &[String]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".into())
}

fn parse_json_array(s: &str) -> Vec<String> {
    serde_json::from_str(s).unwrap_or_default()
}

pub fn get_run(conn: &Connection, run_id: &str) -> Result<Option<RunRow>, StorageError> {
    Ok(conn
        .query_row(
            "SELECT run_id, project_id, repo_path, base_ref, name, status, max_parallel, created_at, updated_at
             FROM orbit_runs WHERE run_id = ?1",
            [run_id],
            row_to_run,
        )
        .optional()?)
}

fn row_to_run(r: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        run_id: r.get(0)?,
        project_id: r.get(1)?,
        repo_path: r.get(2)?,
        base_ref: r.get(3)?,
        name: r.get(4)?,
        status: r.get(5)?,
        max_parallel: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

pub fn list_runs(conn: &Connection, limit: u32) -> Result<Vec<RunRow>, StorageError> {
    let mut s = conn.prepare(
        "SELECT run_id, project_id, repo_path, base_ref, name, status, max_parallel, created_at, updated_at
         FROM orbit_runs ORDER BY rowid DESC LIMIT ?1",
    )?;
    let rows = s.query_map([limit.clamp(1, 200)], row_to_run)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn set_run_status(conn: &Connection, run_id: &str, status: &str) -> Result<(), StorageError> {
    conn.execute(
        &format!("UPDATE orbit_runs SET status = ?2, updated_at = {NOW} WHERE run_id = ?1"),
        params![run_id, status],
    )?;
    Ok(())
}

pub fn load_tasks(conn: &Connection, run_id: &str) -> Result<Vec<TaskRow>, StorageError> {
    let mut s = conn.prepare(
        "SELECT id, task_key, description, command, status, attempts, max_retries, timeout_secs,
                branch, worktree, exit_code, error, changed_files, approval, approval_reason
         FROM tasks WHERE run_id = ?1 ORDER BY id",
    )?;
    let mut tasks: Vec<(i64, TaskRow)> = Vec::new();
    let rows = s.query_map([run_id], |r| {
        let changed: Option<String> = r.get(12)?;
        Ok((
            r.get::<_, i64>(0)?,
            TaskRow {
                key: r.get(1)?,
                description: r.get(2)?,
                argv: parse_json_array(&r.get::<_, String>(3)?),
                status: r.get(4)?,
                attempts: r.get(5)?,
                max_retries: r.get(6)?,
                timeout_secs: r.get(7)?,
                branch: r.get(8)?,
                worktree: r.get(9)?,
                exit_code: r.get(10)?,
                error: r.get(11)?,
                changed_files: changed.as_deref().map(parse_json_array).unwrap_or_default(),
                approval: r.get(13)?,
                approval_reason: r.get(14)?,
                deps: Vec::new(),
            },
        ))
    })?;
    for row in rows {
        tasks.push(row?);
    }
    let mut d = conn.prepare(
        "SELECT a.id, b.task_key FROM task_dependencies td
         JOIN tasks a ON a.id = td.task_id JOIN tasks b ON b.id = td.depends_on
         WHERE a.run_id = ?1 ORDER BY b.id",
    )?;
    let deps = d.query_map([run_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
    })?;
    for dep in deps {
        let (id, key) = dep?;
        if let Some((_, t)) = tasks.iter_mut().find(|(tid, _)| *tid == id) {
            t.deps.push(key);
        }
    }
    Ok(tasks.into_iter().map(|(_, t)| t).collect())
}

pub fn update_task(
    conn: &Connection,
    run_id: &str,
    key: &str,
    p: &TaskPatch,
) -> Result<(), StorageError> {
    let changed = p.changed_files.as_ref().map(|f| {
        let capped: Vec<String> = f.iter().take(200).cloned().collect();
        serde_json_array(&capped)
    });
    let error = p
        .error
        .as_ref()
        .map(|e| redact(e).chars().take(200).collect::<String>());
    conn.execute(
        &format!(
            "UPDATE tasks SET
               status = COALESCE(?3, status), attempts = COALESCE(?4, attempts),
               exit_code = COALESCE(?5, exit_code), error = COALESCE(?6, error),
               branch = COALESCE(?7, branch), worktree = COALESCE(?8, worktree),
               changed_files = COALESCE(?9, changed_files),
               started_at = CASE WHEN ?10 = 1 THEN {NOW} ELSE started_at END,
               ended_at = CASE WHEN ?11 = 1 THEN {NOW} ELSE ended_at END
             WHERE run_id = ?1 AND task_key = ?2"
        ),
        params![
            run_id,
            key,
            p.status,
            p.attempts,
            p.exit_code,
            error,
            p.branch,
            p.worktree,
            changed,
            i64::from(p.mark_started),
            i64::from(p.mark_ended)
        ],
    )?;
    Ok(())
}

/// Grant or deny a task that is `awaiting_approval`. Returns `false` when it was not waiting.
pub fn resolve_approval(
    conn: &Connection,
    run_id: &str,
    key: &str,
    grant: bool,
) -> Result<bool, StorageError> {
    let n = conn.execute(
        "UPDATE tasks SET approval = ?3, status = ?4
         WHERE run_id = ?1 AND task_key = ?2 AND status = 'awaiting_approval'",
        params![
            run_id,
            key,
            if grant { "granted" } else { "denied" },
            if grant { "pending" } else { "denied" }
        ],
    )?;
    Ok(n > 0)
}

pub fn delete_run(conn: &Connection, run_id: &str) -> Result<(), StorageError> {
    conn.execute(
        "DELETE FROM task_dependencies WHERE task_id IN (SELECT id FROM tasks WHERE run_id = ?1)",
        [run_id],
    )?;
    conn.execute("DELETE FROM tasks WHERE run_id = ?1", [run_id])?;
    conn.execute("DELETE FROM orbit_runs WHERE run_id = ?1", [run_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate;

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c
    }

    fn run(id: &str) -> NewRun {
        NewRun {
            run_id: id.into(),
            project_id: "p".into(),
            repo_path: "/r".into(),
            base_ref: "abc".into(),
            name: "n".into(),
            max_parallel: 2,
        }
    }

    fn task(key: &str, deps: &[&str], approval: bool) -> NewTask {
        NewTask {
            key: key.into(),
            description: String::new(),
            argv: vec!["echo".into(), "he said \"hi\"\\ \n".into()],
            timeout_secs: 60,
            max_retries: 1,
            needs_approval: approval,
            approval_reason: if approval {
                "destructive".into()
            } else {
                String::new()
            },
            deps: deps.iter().map(|s| (*s).into()).collect(),
        }
    }

    #[test]
    fn runs_tasks_and_dependencies_round_trip() {
        let c = db();
        create_run(
            &c,
            &run("r1"),
            &[task("a", &[], false), task("b", &["a"], true)],
        )
        .unwrap();
        let tasks = load_tasks(&c, "r1").unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            tasks[0].argv,
            vec!["echo".to_string(), "he said \"hi\"\\ \n".to_string()]
        );
        assert_eq!(
            (tasks[0].status.as_str(), tasks[0].approval.as_str()),
            ("pending", "none")
        );
        assert_eq!(
            (
                tasks[1].status.as_str(),
                tasks[1].approval.as_str(),
                tasks[1].deps.clone()
            ),
            ("awaiting_approval", "required", vec!["a".to_string()])
        );
        assert_eq!(get_run(&c, "r1").unwrap().unwrap().status, "created");
        assert!(get_run(&c, "nope").unwrap().is_none());
    }

    #[test]
    fn approval_can_be_granted_or_denied_once() {
        let c = db();
        create_run(
            &c,
            &run("r1"),
            &[task("a", &[], true), task("b", &[], true)],
        )
        .unwrap();
        assert!(resolve_approval(&c, "r1", "a", true).unwrap());
        assert!(
            !resolve_approval(&c, "r1", "a", false).unwrap(),
            "already resolved"
        );
        assert!(resolve_approval(&c, "r1", "b", false).unwrap());
        let t = load_tasks(&c, "r1").unwrap();
        assert_eq!(
            (t[0].status.as_str(), t[0].approval.as_str()),
            ("pending", "granted")
        );
        assert_eq!(
            (t[1].status.as_str(), t[1].approval.as_str()),
            ("denied", "denied")
        );
    }

    #[test]
    fn patches_update_only_given_fields_and_redact_errors() {
        let c = db();
        create_run(&c, &run("r1"), &[task("a", &[], false)]).unwrap();
        update_task(
            &c,
            "r1",
            "a",
            &TaskPatch {
                status: Some("running"),
                attempts: Some(1),
                mark_started: true,
                ..Default::default()
            },
        )
        .unwrap();
        update_task(
            &c,
            "r1",
            "a",
            &TaskPatch {
                status: Some("failed"),
                exit_code: Some(2),
                error: Some(format!("boom ghp_{}", "a1B2".repeat(9))),
                changed_files: Some(vec!["a.rs".into(), "b c.rs".into()]),
                mark_ended: true,
                ..Default::default()
            },
        )
        .unwrap();
        let t = &load_tasks(&c, "r1").unwrap()[0];
        assert_eq!(
            (t.status.as_str(), t.attempts, t.exit_code),
            ("failed", 1, Some(2))
        );
        assert!(!t.error.clone().unwrap().contains("ghp_a1B2"));
        assert_eq!(
            t.changed_files,
            vec!["a.rs".to_string(), "b c.rs".to_string()]
        );
    }

    #[test]
    fn duplicate_keys_and_oversized_plans_are_refused_and_old_runs_pruned() {
        let c = db();
        assert!(create_run(
            &c,
            &run("r1"),
            &[task("a", &[], false), task("a", &[], false)]
        )
        .is_err());
        let many: Vec<NewTask> = (0..=MAX_TASKS_PER_RUN)
            .map(|i| task(&format!("t{i}"), &[], false))
            .collect();
        assert!(create_run(&c, &run("r2"), &many).is_err());
        for i in 0..(MAX_RUNS + 5) {
            create_run(&c, &run(&format!("x{i}")), &[]).unwrap();
        }
        assert_eq!(list_runs(&c, 200).unwrap().len(), 200);
        assert!(get_run(&c, "x0").unwrap().is_none());
        delete_run(&c, "x204").unwrap();
        assert!(get_run(&c, "x204").unwrap().is_none());
    }
}
