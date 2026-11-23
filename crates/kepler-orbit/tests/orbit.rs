#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Orbit end to end on temporary git repositories. Tasks are this test binary re-executed
//! (`fake_task_entry`), driven by `FAKE_<TASK>` environment variables, so no shell or external tool
//! (besides git) is needed and the same tests run on Windows and Unix.

use kepler_orbit::{parse_plan, validate, Orbit, OrbitError, RunSummary};
use kepler_policy::Guardian;
use kepler_storage::Writer;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

static N: AtomicU32 = AtomicU32::new(0);

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

/// Entry point of a fake task. Ops (separated by `;`): `log=<file>`, `write=<file>:<text>`,
/// `sleep=<ms>`, `failonce=<marker>`, `exit=<code>`.
#[test]
fn fake_task_entry() {
    let Ok(key) = std::env::var("KEPLER_ORBIT_TASK") else {
        return;
    };
    let spec = std::env::var(format!("FAKE_{}", key.to_uppercase())).unwrap_or_default();
    let ops: Vec<&str> = spec.split(';').filter(|s| !s.is_empty()).collect();
    let log = ops.iter().find_map(|o| o.strip_prefix("log="));
    let append = |what: &str| {
        if let Some(f) = log {
            use std::io::Write;
            let mut h = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(f)
                .unwrap();
            // One write call per line: `writeln!` issues several, which concurrent tasks interleave.
            let line = format!("{key} {what} {}\n", now_ms());
            h.write_all(line.as_bytes()).unwrap();
        }
    };
    append("start");
    for op in &ops {
        if let Some(rest) = op.strip_prefix("write=") {
            let (file, text) = rest.split_once(':').unwrap();
            std::fs::write(file, text).unwrap();
        } else if let Some(ms) = op.strip_prefix("sleep=") {
            std::thread::sleep(Duration::from_millis(ms.parse().unwrap()));
        } else if let Some(marker) = op.strip_prefix("failonce=") {
            if !Path::new(marker).exists() {
                std::fs::write(marker, "x").unwrap();
                std::process::exit(1);
            }
        } else if let Some(code) = op.strip_prefix("exit=") {
            std::process::exit(code.parse().unwrap());
        }
    }
    append("end");
    std::process::exit(0);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

struct Env {
    repo: PathBuf,
    orbit: Orbit,
    tmp: PathBuf,
}

fn env() -> Env {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let tmp = std::env::temp_dir().join(format!("kepler-orbit-it-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("a.txt"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let writer = Arc::new(Writer::start(&tmp.join("k.db"), Default::default()).unwrap());
    Env {
        repo,
        orbit: Orbit::new(writer, tmp.join("orbit")),
        tmp,
    }
}

fn argv() -> Vec<String> {
    [
        std::env::current_exe().unwrap().display().to_string(),
        "--exact".into(),
        "fake_task_entry".into(),
        "--nocapture".into(),
        "--test-threads=1".into(),
    ]
    .to_vec()
}

fn task(id: &str, deps: &[&str], behavior: &str) -> serde_json::Value {
    std::env::set_var(format!("FAKE_{}", id.to_uppercase()), behavior);
    serde_json::json!({"id": id, "command": argv(), "depends_on": deps})
}

fn start(e: &Env, tasks: Vec<serde_json::Value>, parallel: u32) -> String {
    let text =
        serde_json::json!({"name": "t", "max_parallel": parallel, "tasks": tasks}).to_string();
    let plan = validate(&parse_plan(&text).unwrap(), &Guardian::default()).unwrap();
    e.orbit.create_run(&e.repo, "HEAD", "proj", &plan).unwrap()
}

fn status_of(s: &RunSummary, key: &str) -> String {
    s.tasks
        .iter()
        .find(|t| t.key == key)
        .unwrap()
        .status
        .clone()
}

fn intervals(log: &Path) -> Vec<(String, u128, u128)> {
    let text = std::fs::read_to_string(log).unwrap();
    let mut starts: Vec<(String, u128)> = Vec::new();
    let mut out = Vec::new();
    for l in text.lines() {
        let p: Vec<&str> = l.split(' ').collect();
        let ms: u128 = p[2].parse().unwrap();
        if p[1] == "start" {
            starts.push((p[0].into(), ms));
        } else if let Some(i) = starts.iter().position(|(k, _)| k == p[0]) {
            let (k, s) = starts.remove(i);
            out.push((k, s, ms));
        }
    }
    out
}

#[test]
fn dag_order_worktree_isolation_and_merge_of_dependencies() {
    let e = env();
    let log = e.tmp.join("order.log");
    let l = format!("log={}", log.display());
    let run = start(
        &e,
        vec![
            task("oa", &[], &format!("{l};write=oa.txt:A;sleep=150")),
            task("ob", &["oa"], &format!("{l};write=ob.txt:B")),
            task("oc", &["oa"], &format!("{l};write=oc.txt:C")),
            task("od", &["ob", "oc"], &format!("{l};write=od.txt:D")),
        ],
        2,
    );
    let s = e.orbit.execute(&run).unwrap();
    assert_eq!(s.run.status, "done", "{:?}", s.tasks);
    assert!(s
        .tasks
        .iter()
        .all(|t| t.status == "done" && t.exit_code == Some(0)));

    // Dependencies ran first.
    let iv = intervals(&log);
    let get = |k: &str| iv.iter().find(|(x, _, _)| x == k).unwrap().clone();
    assert!(get("ob").1 >= get("oa").2 && get("oc").1 >= get("oa").2);
    assert!(get("od").1 >= get("ob").2 && get("od").1 >= get("oc").2);

    // Each task worked in its own worktree; `od` sees its dependencies' work (merged), siblings do not.
    let wt = |k: &str| {
        PathBuf::from(
            s.tasks
                .iter()
                .find(|t| t.key == k)
                .unwrap()
                .worktree
                .clone()
                .unwrap(),
        )
    };
    for f in ["oa.txt", "ob.txt", "oc.txt", "od.txt"] {
        assert!(wt("od").join(f).exists(), "{f} missing in od's worktree");
    }
    assert!(wt("ob").join("oa.txt").exists() && !wt("ob").join("oc.txt").exists());
    let a = s.tasks.iter().find(|t| t.key == "oa").unwrap();
    assert_eq!(a.changed_files, vec!["oa.txt"]);
    assert!(s.overlaps.is_empty());

    // The user's working copy and branch are untouched.
    for f in ["oa.txt", "ob.txt", "oc.txt", "od.txt"] {
        assert!(!e.repo.join(f).exists());
    }
    assert_eq!(git(&e.repo, &["status", "--porcelain"]).trim(), "");
    let branches = git(&e.repo, &["branch", "--list", "kepler/orbit/*"]);
    assert!(
        branches.contains(&format!("kepler/orbit/{run}/od")),
        "{branches}"
    );

    // Cleanup removes worktrees, branches and logs.
    assert_eq!(e.orbit.cleanup(&run, true).unwrap(), 4);
    assert!(!wt("od").exists());
    assert_eq!(
        git(&e.repo, &["branch", "--list", "kepler/orbit/*"]).trim(),
        ""
    );
    assert!(matches!(e.orbit.status(&run), Err(OrbitError::NotFound)));
}

#[test]
fn concurrency_is_bounded_by_max_parallel() {
    let e = env();
    let log = e.tmp.join("par.log");
    let l = format!("log={}", log.display());
    let tasks: Vec<_> = (0..6)
        .map(|i| task(&format!("pc{i}"), &[], &format!("{l};sleep=300")))
        .collect();
    let run = start(&e, tasks, 2);
    let s = e.orbit.execute(&run).unwrap();
    assert_eq!(s.run.status, "done");
    let iv = intervals(&log);
    assert_eq!(iv.len(), 6);
    let max_overlap = iv
        .iter()
        .map(|(_, s0, _)| iv.iter().filter(|(_, s1, e1)| s1 <= s0 && s0 < e1).count())
        .max()
        .unwrap();
    assert_eq!(
        max_overlap, 2,
        "never more than max_parallel at once, and the bound is used"
    );
}

#[test]
fn failures_skip_dependents_retries_recover_and_timeouts_kill() {
    let e = env();
    let marker = e.tmp.join("once.marker");
    let run = start(
        &e,
        vec![
            task("fx", &[], "exit=3"),
            task("fy", &["fx"], "write=y.txt:y"),
            task("fz", &[], "write=z.txt:z"),
            {
                let mut t = task("fr", &[], &format!("failonce={}", marker.display()));
                t["max_retries"] = 1.into();
                t
            },
            {
                let mut t = task("ft", &[], "sleep=20000");
                t["timeout_secs"] = 1.into();
                t
            },
        ],
        4,
    );
    let began = Instant::now();
    let s = e.orbit.execute(&run).unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(15),
        "{:?}",
        began.elapsed()
    );
    assert_eq!(s.run.status, "failed");
    assert_eq!(status_of(&s, "fx"), "failed");
    assert_eq!(
        s.tasks.iter().find(|t| t.key == "fx").unwrap().exit_code,
        Some(3)
    );
    assert_eq!(
        status_of(&s, "fy"),
        "skipped",
        "dependents of a failure never run"
    );
    assert_eq!(
        status_of(&s, "fz"),
        "done",
        "independent work still completes"
    );
    let r = s.tasks.iter().find(|t| t.key == "fr").unwrap();
    assert_eq!(
        (r.status.as_str(), r.attempts),
        ("done", 2),
        "one bounded retry recovered it"
    );
    let t = s.tasks.iter().find(|t| t.key == "ft").unwrap();
    assert_eq!(t.status, "failed");
    assert!(
        t.error.as_deref().unwrap().contains("timeout"),
        "{:?}",
        t.error
    );
}

#[test]
fn destructive_commands_wait_for_approval_and_resume_after_it() {
    let e = env();
    let text = serde_json::json!({"name": "t", "tasks": [
        {"id": "ap1", "command": ["git", "clean", "-fd"]},
        {"id": "ap2", "command": ["git", "status"], "depends_on": ["ap1"]},
        {"id": "ap3", "command": ["git", "status"]},
    ]})
    .to_string();
    let plan = validate(&parse_plan(&text).unwrap(), &Guardian::default()).unwrap();
    let run = e.orbit.create_run(&e.repo, "HEAD", "proj", &plan).unwrap();

    let s = e.orbit.execute(&run).unwrap();
    assert_eq!(s.run.status, "needs_approval");
    assert_eq!(
        status_of(&s, "ap1"),
        "awaiting_approval",
        "destructive command did not run"
    );
    assert_eq!(status_of(&s, "ap2"), "pending", "dependent waits");
    assert_eq!(
        status_of(&s, "ap3"),
        "done",
        "harmless independent work proceeds"
    );
    assert!(
        s.tasks[0].worktree.is_none(),
        "no worktree was even created"
    );

    assert!(e.orbit.resolve_approval(&run, "ap1", true).unwrap());
    assert!(
        !e.orbit.resolve_approval(&run, "ap1", true).unwrap(),
        "approval is one-shot"
    );
    let s = e.orbit.execute(&run).unwrap();
    assert_eq!(s.run.status, "done", "{:?}", s.tasks);
    assert_eq!(s.tasks[0].approval, "granted");

    // Denial fails the task and skips what depends on it.
    let run2 = e.orbit.create_run(&e.repo, "HEAD", "proj", &plan).unwrap();
    e.orbit.execute(&run2).unwrap();
    assert!(e.orbit.resolve_approval(&run2, "ap1", false).unwrap());
    let s = e.orbit.execute(&run2).unwrap();
    assert_eq!(s.run.status, "failed");
    assert_eq!(status_of(&s, "ap1"), "denied");
    assert_eq!(status_of(&s, "ap2"), "skipped");
}

#[test]
fn overlapping_independent_changes_are_reported_and_merge_conflicts_fail_the_dependent() {
    let e = env();
    let run = start(
        &e,
        vec![
            task("cx", &[], "write=a.txt:from-x"),
            task("cy", &[], "write=a.txt:from-y"),
            task("cd", &["cx", "cy"], "write=d.txt:d"),
        ],
        2,
    );
    let s = e.orbit.execute(&run).unwrap();
    assert_eq!(s.run.status, "failed");
    assert_eq!(
        (status_of(&s, "cx"), status_of(&s, "cy")),
        ("done".into(), "done".into())
    );
    assert_eq!(s.overlaps.len(), 1, "{:?}", s.overlaps);
    assert_eq!(s.overlaps[0].files, vec!["a.txt"]);
    let d = s.tasks.iter().find(|t| t.key == "cd").unwrap();
    assert_eq!(d.status, "failed");
    assert!(
        d.error.as_deref().unwrap().starts_with("merge_conflict"),
        "{:?}",
        d.error
    );
    assert_eq!(
        std::fs::read_to_string(e.repo.join("a.txt")).unwrap(),
        "base\n"
    );
}

#[test]
fn cancel_kills_running_tasks_and_a_second_executor_is_refused() {
    let e = Arc::new(env());
    let run = start(
        &e,
        vec![
            task("kx", &[], "sleep=30000"),
            task("ky", &["kx"], "write=y:y"),
        ],
        2,
    );
    let (e2, run2) = (e.clone(), run.clone());
    let worker = std::thread::spawn(move || e2.orbit.execute(&run2).unwrap());
    let began = Instant::now();
    loop {
        let s = e.orbit.status(&run).unwrap();
        if s.tasks[0].status == "running" {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "task never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // While it runs, a second executor is refused (run status + lock file).
    assert!(matches!(e.orbit.execute(&run), Err(OrbitError::State(_))));
    e.orbit.request_cancel(&run).unwrap();
    let s = worker.join().unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "{:?}",
        began.elapsed()
    );
    assert_eq!(s.run.status, "cancelled");
    assert_eq!(status_of(&s, "kx"), "cancelled");
    assert_eq!(
        status_of(&s, "ky"),
        "skipped",
        "its dependency was cancelled"
    );
    assert!(
        e.orbit.request_cancel(&run).is_err(),
        "nothing left to cancel"
    );
    // Regression: a cancelled task never recorded its worktree/branch, and cleanup used to skip
    // it, leaking a prunable worktree registration and the branch.
    assert!(
        git(&e.repo, &["branch", "--list", "kepler/orbit/*"]).contains("/kx"),
        "the cancelled task's branch exists before cleanup"
    );
    e.orbit.cleanup(&run, true).unwrap();
    assert_eq!(
        git(&e.repo, &["branch", "--list", "kepler/orbit/*"]).trim(),
        ""
    );
    assert_eq!(
        git(&e.repo, &["worktree", "list"]).lines().count(),
        1,
        "only the main worktree remains"
    );
}

#[test]
fn plans_with_cycles_or_denied_commands_never_create_runs() {
    let g = Guardian::default();
    let cyc = r#"{"name":"t","tasks":[{"id":"a","command":["git","status"],"depends_on":["b"]},{"id":"b","command":["git","status"],"depends_on":["a"]}]}"#;
    assert!(validate(&parse_plan(cyc).unwrap(), &g).is_err());
    let denied = r#"{"name":"t","tasks":[{"id":"a","command":["cat",".env"]}]}"#;
    assert!(validate(&parse_plan(denied).unwrap(), &g).is_err());
}
