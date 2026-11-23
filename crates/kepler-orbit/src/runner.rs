//! Scheduler and child-process supervision. One orchestrating thread (the caller) owns the
//! state machine; each running task has one thread; at most `max_parallel` run at once.

use crate::plan::ValidPlan;
use crate::worktree;
use kepler_policy::Guardian;
use kepler_storage::redact::redact;
use kepler_storage::{
    create_run, delete_run, get_run, load_tasks, resolve_approval, set_run_status, update_task,
    Connection, NewRun, RunRow, StorageError, TaskPatch, TaskRow, Writer,
};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

const LOG_CAP: usize = 1024 * 1024;
const DB_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum OrbitError {
    Storage(StorageError),
    Git(String),
    NotFound,
    State(String),
    Io(String),
}

impl std::fmt::Display for OrbitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(e) => write!(f, "{e}"),
            Self::Git(e) => write!(f, "git: {e}"),
            Self::NotFound => write!(f, "no such run or task"),
            Self::State(e) | Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OrbitError {}

impl From<StorageError> for OrbitError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    pub a: String,
    pub b: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RunSummary {
    pub run: RunRow,
    pub tasks: Vec<TaskRow>,
    /// Independent tasks (no dependency path between them) that changed the same file: merging
    /// their branches will conflict or silently pick one side, so a human should look.
    pub overlaps: Vec<Overlap>,
}

pub struct Orbit {
    writer: Arc<Writer>,
    root: PathBuf,
    guardian: Guardian,
    cancel_poll: Duration,
}

static RUN_COUNTER: AtomicU32 = AtomicU32::new(0);

fn exec<T, F>(w: &Writer, f: F) -> Result<T, OrbitError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
{
    Ok(w.exec(DB_TIMEOUT, f)?)
}

fn display_path(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_string()
}

fn blocks_dependents(status: &str) -> bool {
    matches!(status, "failed" | "cancelled" | "skipped" | "denied")
}

#[derive(Debug, Clone)]
struct TaskState {
    key: String,
    argv: Vec<String>,
    deps: Vec<String>,
    status: String,
    attempts: u32,
    max_retries: u32,
    timeout_secs: u32,
    /// Branch holding this task's committed work (set when it produced changes).
    work_branch: Option<String>,
}

enum Outcome {
    Done {
        branch: String,
        worktree: String,
        changed: Vec<String>,
        committed: bool,
    },
    Failed {
        exit_code: Option<i32>,
        error: String,
        branch: Option<String>,
        worktree: Option<String>,
    },
    Cancelled,
}

struct Shared {
    root: PathBuf,
    repo: PathBuf,
    base_sha: String,
    run_id: String,
    cancel: AtomicBool,
}

fn kill_tree(child: &mut Child) {
    let pid = child.id().to_string();
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid, "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = pid;
    let _ = child.kill();
    let _ = child.wait();
}

/// Copy a pipe into a capped log file. Detached: it ends when the pipe closes.
fn capture(
    mut pipe: impl Read + Send + 'static,
    mut log: std::fs::File,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut written = 0usize;
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            if written < LOG_CAP {
                let take = n.min(LOG_CAP - written);
                let _ = log.write_all(&buf[..take]);
                written += take;
            }
        }
    })
}

/// Wait (briefly) for the capture threads, then redact the log in place: task output may contain
/// secrets (an agent can print anything), and the log is the only place it is kept.
fn finalize_log(handles: Vec<std::thread::JoinHandle<()>>, paths: &[PathBuf]) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !handles.iter().all(std::thread::JoinHandle::is_finished) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for path in paths {
        if let Ok(bytes) = std::fs::read(path) {
            let text = String::from_utf8_lossy(&bytes);
            let clean = redact(&text);
            if clean != text {
                let _ = std::fs::write(path, clean.as_bytes());
            }
        }
    }
}

fn run_task(sh: &Shared, t: &TaskState, dep_branches: &[String]) -> Outcome {
    let mut handles = Vec::new();
    let out = run_task_inner(sh, t, dep_branches, &mut handles);
    let dir = sh.root.join(&sh.run_id);
    finalize_log(
        handles,
        &[
            dir.join(format!("{}.log", t.key)),
            dir.join(format!("{}.err.log", t.key)),
        ],
    );
    out
}

fn run_task_inner(
    sh: &Shared,
    t: &TaskState,
    dep_branches: &[String],
    handles: &mut Vec<std::thread::JoinHandle<()>>,
) -> Outcome {
    let wt = sh.root.join(&sh.run_id).join(&t.key);
    let branch = format!("kepler/orbit/{}/{}", sh.run_id, t.key);
    // A retry starts from a clean slate.
    if wt.exists() {
        worktree::remove_worktree(&sh.repo, &wt, &branch);
    }
    let fail = |error: String, exit_code: Option<i32>, has_wt: bool| Outcome::Failed {
        exit_code,
        error,
        branch: has_wt.then(|| branch.clone()),
        worktree: has_wt.then(|| display_path(&wt)),
    };
    if let Err(e) = worktree::add_worktree(&sh.repo, &wt, &branch, &sh.base_sha) {
        return fail(format!("worktree: {e}"), None, false);
    }
    if let Err(e) = worktree::merge_branches(&wt, dep_branches) {
        return fail(e, None, true);
    }
    let log_path = sh.root.join(&sh.run_id).join(format!("{}.log", t.key));
    let err_path = sh.root.join(&sh.run_id).join(format!("{}.err.log", t.key));
    let Some(program) = t.argv.first() else {
        return fail("empty command".into(), None, true);
    };
    let mut cmd = Command::new(program);
    cmd.args(&t.argv[1..])
        .current_dir(&wt)
        .env("KEPLER_ORBIT_RUN", &sh.run_id)
        .env("KEPLER_ORBIT_TASK", &t.key)
        .env("KEPLER_ORBIT_WORKTREE", &wt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("cannot start `{program}`: {e}"), None, true),
    };
    for (pipe, label) in [
        (
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            "stdout",
        ),
        (
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn Read + Send>),
            "stderr",
        ),
    ] {
        if let (Some(p), Ok(f)) = (
            pipe,
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(if label == "stderr" {
                    &err_path
                } else {
                    &log_path
                }),
        ) {
            handles.push(capture(p, f));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(u64::from(t.timeout_secs));
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                kill_tree(&mut child);
                return fail(format!("wait failed: {e}"), None, true);
            }
        }
        if sh.cancel.load(Ordering::Relaxed) {
            kill_tree(&mut child);
            return Outcome::Cancelled;
        }
        if Instant::now() >= deadline {
            kill_tree(&mut child);
            return fail(format!("timeout after {} s", t.timeout_secs), None, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !status.success() {
        return fail(
            format!(
                "exit status {}",
                status.code().map_or("signal".into(), |c| c.to_string())
            ),
            status.code(),
            true,
        );
    }
    let changed = worktree::changed_files(&wt);
    match worktree::commit_all(&wt, &format!("orbit: {} ({})", t.key, sh.run_id)) {
        Ok(committed) => Outcome::Done {
            branch,
            worktree: display_path(&wt),
            changed,
            committed,
        },
        Err(e) => fail(format!("commit: {e}"), status.code(), true),
    }
}

impl Orbit {
    /// `root` holds worktrees and logs (normally `<data dir>/orbit`).
    pub fn new(writer: Arc<Writer>, root: PathBuf) -> Self {
        Self {
            writer,
            root,
            guardian: Guardian::default(),
            cancel_poll: Duration::from_millis(250),
        }
    }

    pub fn with_guardian(mut self, g: Guardian) -> Self {
        self.guardian = g;
        self
    }

    pub fn guardian(&self) -> &Guardian {
        &self.guardian
    }

    /// Persist a validated plan as a new run based on `base_ref` of `repo`. Nothing runs yet.
    pub fn create_run(
        &self,
        repo: &Path,
        base_ref: &str,
        project_id: &str,
        plan: &ValidPlan,
    ) -> Result<String, OrbitError> {
        let repo = std::fs::canonicalize(repo).map_err(|e| OrbitError::Io(e.to_string()))?;
        let repo = PathBuf::from(display_path(&repo));
        if !kepler_git::is_repo(&repo) {
            return Err(OrbitError::Git("not inside a git work tree".into()));
        }
        let base = worktree::resolve_base(&repo, base_ref).map_err(OrbitError::Git)?;
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let run_id = format!("orb-{ms:x}-{}", RUN_COUNTER.fetch_add(1, Ordering::SeqCst));
        let run = NewRun {
            run_id: run_id.clone(),
            project_id: project_id.into(),
            repo_path: repo.display().to_string(),
            base_ref: base,
            name: plan.name.clone(),
            max_parallel: plan.max_parallel,
        };
        let tasks = plan.tasks.clone();
        exec(&self.writer, move |c| create_run(c, &run, &tasks))?;
        Ok(run_id)
    }

    fn load(&self, run_id: &str) -> Result<(RunRow, Vec<TaskRow>), OrbitError> {
        let id = run_id.to_string();
        exec(&self.writer, move |c| {
            let run = get_run(c, &id)?;
            let tasks = if run.is_some() {
                load_tasks(c, &id)?
            } else {
                Vec::new()
            };
            Ok((run, tasks))
        })
        .and_then(|(r, t)| r.map(|r| (r, t)).ok_or(OrbitError::NotFound))
    }

    pub fn status(&self, run_id: &str) -> Result<RunSummary, OrbitError> {
        let (run, tasks) = self.load(run_id)?;
        let overlaps = overlaps(&tasks);
        Ok(RunSummary {
            run,
            tasks,
            overlaps,
        })
    }

    /// Grant or deny a task waiting for approval. `Ok(false)` when it was not waiting.
    pub fn resolve_approval(
        &self,
        run_id: &str,
        key: &str,
        grant: bool,
    ) -> Result<bool, OrbitError> {
        let (id, k) = (run_id.to_string(), key.to_string());
        exec(&self.writer, move |c| resolve_approval(c, &id, &k, grant))
    }

    /// Ask a running `execute` (in any process) to stop: running tasks are killed, pending ones
    /// are marked cancelled.
    pub fn request_cancel(&self, run_id: &str) -> Result<(), OrbitError> {
        let (run, _) = self.load(run_id)?;
        if run.status != "running" {
            return Err(OrbitError::State(format!(
                "run is {}, not running",
                run.status
            )));
        }
        let id = run_id.to_string();
        exec(&self.writer, move |c| set_run_status(c, &id, "cancelling"))
    }

    /// Remove worktrees, branches and logs of a run (and optionally its database rows).
    pub fn cleanup(&self, run_id: &str, delete_records: bool) -> Result<usize, OrbitError> {
        let (run, tasks) = self.load(run_id)?;
        if run.status == "running" {
            return Err(OrbitError::State("run is still running".into()));
        }
        let repo = PathBuf::from(&run.repo_path);
        let mut removed = 0;
        // Path and branch are deterministic, so tasks that never recorded them (cancelled or killed
        // mid-run) are cleaned up too; deriving them avoids leaking worktrees and branches.
        for t in &tasks {
            let wt = self.root.join(run_id).join(&t.key);
            let branch = format!("kepler/orbit/{run_id}/{}", t.key);
            if worktree::remove_worktree(&repo, &wt, &branch) {
                removed += 1;
            }
        }
        let dir = self.root.join(run_id);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        if delete_records {
            let id = run_id.to_string();
            exec(&self.writer, move |c| delete_run(c, &id))?;
        }
        Ok(removed)
    }

    /// Run everything that can run: ready tasks (dependencies done, approvals granted) up to
    /// `max_parallel` at a time. Returns when nothing is running and nothing can start. Safe to call
    /// again after approvals ("resume"). Only one executor per run (lock file).
    pub fn execute(&self, run_id: &str) -> Result<RunSummary, OrbitError> {
        let (run, rows) = self.load(run_id)?;
        if run.status == "running" || run.status == "cancelling" {
            return Err(OrbitError::State(
                "run is marked as running (another process, or a crash; see `kepler orbit status`)"
                    .into(),
            ));
        }
        let run_dir = self.root.join(run_id);
        std::fs::create_dir_all(&run_dir).map_err(|e| OrbitError::Io(e.to_string()))?;
        let lock = run_dir.join(".lock");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|_| {
                OrbitError::State(format!(
                    "run is locked by another executor ({})",
                    lock.display()
                ))
            })?;
        let result = self.execute_locked(&run, rows);
        let _ = std::fs::remove_file(&lock);
        result
    }

    fn execute_locked(&self, run: &RunRow, rows: Vec<TaskRow>) -> Result<RunSummary, OrbitError> {
        let run_id = run.run_id.clone();
        let sh = Arc::new(Shared {
            root: self.root.clone(),
            repo: PathBuf::from(&run.repo_path),
            base_sha: run.base_ref.clone(),
            run_id: run_id.clone(),
            cancel: AtomicBool::new(false),
        });
        // Mirror of persisted state. A task left `running` by a crashed executor starts over.
        let mut st: Vec<TaskState> = rows
            .iter()
            .map(|r| TaskState {
                key: r.key.clone(),
                argv: r.argv.clone(),
                deps: r.deps.clone(),
                status: if r.status == "running" {
                    "pending".into()
                } else {
                    r.status.clone()
                },
                attempts: r.attempts,
                max_retries: r.max_retries,
                timeout_secs: r.timeout_secs,
                work_branch: r
                    .branch
                    .clone()
                    .filter(|_| r.status == "done" && !r.changed_files.is_empty()),
            })
            .collect();
        self.set_status(&run_id, "running")?;
        let (tx, rx) = channel::<(String, Outcome)>();
        let mut running: HashMap<String, std::thread::JoinHandle<()>> = HashMap::new();
        let mut last_cancel_check = Instant::now();
        let mut cancelling = false;

        loop {
            if last_cancel_check.elapsed() >= self.cancel_poll && !cancelling {
                last_cancel_check = Instant::now();
                let id = run_id.clone();
                let status = exec(
                    &self.writer,
                    move |c| Ok(get_run(c, &id)?.map(|r| r.status)),
                )?;
                if status.as_deref() == Some("cancelling") {
                    cancelling = true;
                    sh.cancel.store(true, Ordering::Relaxed);
                }
            }
            // Dependents of a failed/cancelled/denied task will never run.
            loop {
                let blocked: Vec<usize> = (0..st.len())
                    .filter(|&i| {
                        st[i].status == "pending"
                            && st[i].deps.iter().any(|d| {
                                st.iter()
                                    .any(|o| o.key == *d && blocks_dependents(&o.status))
                            })
                    })
                    .collect();
                if blocked.is_empty() {
                    break;
                }
                for i in blocked {
                    "skipped".clone_into(&mut st[i].status);
                    self.patch(
                        &run_id,
                        &st[i].key,
                        TaskPatch {
                            status: Some("skipped"),
                            error: Some("a dependency did not complete".into()),
                            mark_ended: true,
                            ..Default::default()
                        },
                    )?;
                }
            }
            if !cancelling {
                let ready: Vec<usize> = (0..st.len())
                    .filter(|&i| {
                        st[i].status == "pending"
                            && st[i]
                                .deps
                                .iter()
                                .all(|d| st.iter().any(|o| o.key == *d && o.status == "done"))
                    })
                    .collect();
                for i in ready {
                    if running.len() >= run.max_parallel.max(1) as usize {
                        break;
                    }
                    st[i].attempts += 1;
                    "running".clone_into(&mut st[i].status);
                    self.patch(
                        &run_id,
                        &st[i].key,
                        TaskPatch {
                            status: Some("running"),
                            attempts: Some(st[i].attempts),
                            mark_started: true,
                            ..Default::default()
                        },
                    )?;
                    let dep_branches: Vec<String> = st[i]
                        .deps
                        .iter()
                        .filter_map(|d| {
                            st.iter()
                                .find(|o| o.key == *d)
                                .and_then(|o| o.work_branch.clone())
                        })
                        .collect();
                    let (shc, task, txc) = (sh.clone(), st[i].clone(), tx.clone());
                    let key = task.key.clone();
                    running.insert(
                        key.clone(),
                        std::thread::spawn(move || {
                            let out = run_task(&shc, &task, &dep_branches);
                            let _ = txc.send((task.key, out));
                        }),
                    );
                }
            }
            if running.is_empty() {
                let more = !cancelling
                    && st.iter().any(|t| {
                        t.status == "pending"
                            && t.deps
                                .iter()
                                .all(|d| st.iter().any(|o| o.key == *d && o.status == "done"))
                    });
                if !more {
                    break;
                }
                continue;
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok((key, out)) => {
                    if let Some(h) = running.remove(&key) {
                        let _ = h.join();
                    }
                    let Some(i) = st.iter().position(|t| t.key == key) else {
                        continue;
                    };
                    match out {
                        Outcome::Done {
                            branch,
                            worktree,
                            changed,
                            committed,
                        } => {
                            "done".clone_into(&mut st[i].status);
                            st[i].work_branch = committed.then(|| branch.clone());
                            self.patch(
                                &run_id,
                                &key,
                                TaskPatch {
                                    status: Some("done"),
                                    exit_code: Some(0),
                                    branch: Some(branch),
                                    worktree: Some(worktree),
                                    changed_files: Some(changed),
                                    mark_ended: true,
                                    ..Default::default()
                                },
                            )?;
                        }
                        Outcome::Failed {
                            exit_code,
                            error,
                            branch,
                            worktree,
                        } => {
                            let retry = st[i].attempts <= st[i].max_retries && !cancelling;
                            let status = if retry { "pending" } else { "failed" };
                            status.clone_into(&mut st[i].status);
                            self.patch(
                                &run_id,
                                &key,
                                TaskPatch {
                                    status: Some(if retry { "pending" } else { "failed" }),
                                    exit_code,
                                    error: Some(redact(&error)),
                                    branch,
                                    worktree,
                                    mark_ended: !retry,
                                    ..Default::default()
                                },
                            )?;
                        }
                        Outcome::Cancelled => {
                            "cancelled".clone_into(&mut st[i].status);
                            self.patch(
                                &run_id,
                                &key,
                                TaskPatch {
                                    status: Some("cancelled"),
                                    mark_ended: true,
                                    ..Default::default()
                                },
                            )?;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        // Anything still pending after a cancel is cancelled.
        if cancelling {
            for t in st.iter_mut().filter(|t| t.status == "pending") {
                "cancelled".clone_into(&mut t.status);
                self.patch(
                    &run_id,
                    &t.key,
                    TaskPatch {
                        status: Some("cancelled"),
                        mark_ended: true,
                        ..Default::default()
                    },
                )?;
            }
        }
        let final_status = if cancelling {
            "cancelled"
        } else if st.iter().any(|t| {
            matches!(
                t.status.as_str(),
                "failed" | "skipped" | "denied" | "cancelled"
            )
        }) {
            "failed"
        } else if st.iter().any(|t| t.status == "awaiting_approval") {
            "needs_approval"
        } else if st.iter().all(|t| t.status == "done") {
            "done"
        } else {
            "failed"
        };
        self.set_status(&run_id, final_status)?;
        self.status(&run_id)
    }

    fn set_status(&self, run_id: &str, status: &'static str) -> Result<(), OrbitError> {
        let id = run_id.to_string();
        exec(&self.writer, move |c| set_run_status(c, &id, status))
    }

    fn patch(&self, run_id: &str, key: &str, p: TaskPatch) -> Result<(), OrbitError> {
        let (id, k) = (run_id.to_string(), key.to_string());
        exec(&self.writer, move |c| update_task(c, &id, &k, &p))
    }
}

/// Pairs of independent done tasks that changed the same file.
pub fn overlaps(tasks: &[TaskRow]) -> Vec<Overlap> {
    let by_key: HashMap<&str, &TaskRow> = tasks.iter().map(|t| (t.key.as_str(), t)).collect();
    fn ancestors<'a>(k: &'a str, by: &HashMap<&'a str, &'a TaskRow>, out: &mut Vec<&'a str>) {
        if let Some(t) = by.get(k) {
            for d in &t.deps {
                if !out.contains(&d.as_str()) {
                    out.push(d);
                    ancestors(d, by, out);
                }
            }
        }
    }
    let mut res = Vec::new();
    for (i, a) in tasks.iter().enumerate() {
        for b in &tasks[i + 1..] {
            if a.status != "done" || b.status != "done" {
                continue;
            }
            let (mut aa, mut bb) = (Vec::new(), Vec::new());
            ancestors(&a.key, &by_key, &mut aa);
            ancestors(&b.key, &by_key, &mut bb);
            if aa.contains(&b.key.as_str()) || bb.contains(&a.key.as_str()) {
                continue;
            }
            let files: Vec<String> = a
                .changed_files
                .iter()
                .filter(|f| b.changed_files.contains(f))
                .cloned()
                .collect();
            if !files.is_empty() {
                res.push(Overlap {
                    a: a.key.clone(),
                    b: b.key.clone(),
                    files,
                });
            }
        }
    }
    res
}
