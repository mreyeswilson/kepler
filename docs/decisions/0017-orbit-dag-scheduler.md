# ADR-0017: Orbit DAG scheduler with git worktree isolation

**Status:** Accepted, implemented in `kepler-orbit`, `kepler-storage` (migration v4) and `kepler orbit ...`

## Context
Delivery 5 asks for a DAG scheduler, isolated worktrees and agent lifecycle support. Orbit must not spawn
agents "because they are installed", must bound concurrency, and must never run destructive actions without
approval (AGENTS.md).

## Decision
* **Plan**: a JSON file (`name`, optional `max_parallel` 1 to 4, `tasks[]` with `id`, `command` as an **argv array**,
  `depends_on`, `timeout_secs` (default 600, max 3600), `max_retries` (0 to 3), `require_approval`). Unknown fields
  are rejected. Limits: 64 tasks, 16 dependencies per task, 32 arguments of 1024 bytes, 256 KiB file.
  Validation detects unknown/duplicate/self dependencies and **cycles** (Kahn's algorithm; the cycle path is
  reported by depth-first search).
* **Safety at plan time**: the joined command goes through the Guardian preflight with the user's rules. `deny`
  invalidates the whole plan (fail closed); `require_approval` (force pushes, `rm -rf`, `git clean`, publishes, ...)
  makes the task start as `awaiting_approval`; a secret-shaped value in a command is refused (use the environment).
* **Approvals**: `kepler orbit approve|deny <run> <task>` then `resume`. Approval is one-shot, per task, and a
  denied task fails and skips its dependents. Nothing destructive runs while waiting; independent harmless work
  proceeds, and the run ends as `needs_approval` (CLI exit 3).
* **Isolation**: every task runs in its own worktree and branch (`kepler/orbit/<run>/<task>`) under
  `<data dir>/orbit/<run>/`, created from the run's base commit (resolved to a SHA, revision spelling validated).
  The user's working copy and branches are never touched. A successful task's changes are committed on its branch
  (bookkeeping commit, `--no-verify`, fixed identity). A dependent's worktree starts from the base with its
  dependencies' branches merged; a merge conflict aborts the merge and **fails the dependent** (`merge_conflict`).
  Independent tasks that changed the same file are reported as overlaps. Nothing is merged into the user's branch:
  that is a human decision (`git merge kepler/orbit/<run>/<task>`); `cleanup --yes` removes worktrees, branches and logs.
* **Execution**: one orchestrating thread plus one thread per running task, at most `max_parallel` at once; the
  process is killed as a tree on timeout or cancel (`taskkill /T` on Windows, a process group kill on Unix). One
  bounded retry policy per task. Task output goes to a capped (1 MiB) local log file, never to the database.
  A lock file prevents two executors on one run; `kepler orbit cancel <run>` (any terminal) sets `cancelling`,
  which the executor polls every 250 ms **only while a run is executing**.
* **Persistence** (`orbit_runs`, `tasks`, `task_dependencies`): argv, status, attempts, exit code, branch,
  worktree, changed file names (<=200) and a redacted <=200-char error. No output, no environment. Latest 200 runs kept.
* "Agent lifecycle support": Orbit supervises child processes only. A task *may* be an agent CLI invocation, but
  Orbit assumes no harness API and never starts an agent on its own.

## Consequences
Safe parallel work on one repository with auditable state. Limits: processes are not memory- or CPU-limited by
the OS (only timeouts); the Unix process-group kill was **not executed on Unix** by the author (written and
reviewed, CI will run the tests); task logs may contain sensitive output and are local files; a crash leaves the run
marked `running` (a stale lock/status must be cleaned by hand); `git worktree` hooks (post-checkout) of the user's
repository run as usual.

## Alternatives
Merging results automatically (rewrites user branches), a shared worktree (no isolation), shell strings for commands
(injection and weaker destructive-command detection), a resident scheduler daemon (idle cost).
