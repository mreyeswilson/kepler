//! Git worktree isolation: one branch and one worktree per task, outside the repository, so
//! parallel tasks never touch the user's working copy or each other.

use kepler_git::{run, valid_ref, GitError, GitOptions};
use std::path::Path;
use std::time::Duration;

const IDENT: [&str; 4] = [
    "-c",
    "user.name=kepler-orbit",
    "-c",
    "user.email=orbit@kepler.invalid",
];

fn opts() -> GitOptions {
    GitOptions {
        timeout: Duration::from_secs(60),
        max_stdout: 1024 * 1024,
    }
}

fn s(e: GitError) -> String {
    e.to_string()
}

/// Resolve a revision to a full commit SHA (spelling validated first).
pub fn resolve_base(repo: &Path, base_ref: &str) -> Result<String, String> {
    if !valid_ref(base_ref) {
        return Err("invalid base revision".into());
    }
    let spec = format!("{base_ref}^{{commit}}");
    let out = run(repo, &["rev-parse", "--verify", &spec], &opts()).map_err(s)?;
    let sha = out.stdout.trim().to_string();
    if sha.len() < 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("unexpected rev-parse output".into());
    }
    Ok(sha)
}

/// Create `branch` at `base_sha` checked out in a new worktree at `path`.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base_sha: &str) -> Result<(), String> {
    if !valid_ref(branch) || !valid_ref(base_sha) {
        return Err("invalid branch or base".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let p = path.to_string_lossy();
    run(
        repo,
        &["worktree", "add", "-q", "-b", branch, &p, base_sha],
        &opts(),
    )
    .map(|_| ())
    .map_err(s)
}

/// Merge the given task branches into the worktree's branch. On a conflict the merge is aborted
/// and the offending branch is named in the error.
pub fn merge_branches(wt: &Path, branches: &[String]) -> Result<(), String> {
    for b in branches {
        if !valid_ref(b) {
            return Err("invalid branch".into());
        }
        let mut args: Vec<&str> = IDENT.to_vec();
        args.extend(["merge", "--no-edit", "--no-ff", "-q", b.as_str()]);
        if let Err(e) = run(wt, &args, &opts()) {
            let _ = run(wt, &["merge", "--abort"], &opts());
            return Err(format!("merge_conflict with {b}: {}", first_line(&s(e))));
        }
    }
    Ok(())
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

/// Paths with uncommitted changes (including untracked), at most 200.
pub fn changed_files(wt: &Path) -> Vec<String> {
    let Ok(o) = run(
        wt,
        &["status", "--porcelain", "--untracked-files=all"],
        &opts(),
    ) else {
        return Vec::new();
    };
    o.stdout
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let p = &l[3..];
            p.rsplit_once(" -> ")
                .map_or(p, |(_, new)| new)
                .trim_matches('"')
                .to_string()
        })
        .take(200)
        .collect()
}

/// Commit everything in the worktree on its task branch. `Ok(false)` when there was nothing to
/// commit. Hooks are skipped (`--no-verify`): this is Orbit's own bookkeeping commit.
pub fn commit_all(wt: &Path, message: &str) -> Result<bool, String> {
    if changed_files(wt).is_empty() {
        return Ok(false);
    }
    run(wt, &["add", "-A"], &opts()).map_err(s)?;
    let mut args: Vec<&str> = IDENT.to_vec();
    args.extend([
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-q",
        "--no-verify",
        "-m",
        message,
    ]);
    run(wt, &args, &opts()).map_err(s)?;
    Ok(true)
}

/// Remove the worktree and delete its branch (both best effort). Returns whether anything was removed.
pub fn remove_worktree(repo: &Path, path: &Path, branch: &str) -> bool {
    let p = path.to_string_lossy();
    let a = run(repo, &["worktree", "remove", "--force", &p], &opts()).is_ok();
    let _ = run(repo, &["worktree", "prune"], &opts());
    let b = valid_ref(branch) && run(repo, &["branch", "-D", branch], &opts()).is_ok();
    if path.exists() {
        let _ = std::fs::remove_dir_all(path);
    }
    a || b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(IDENT)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    pub(crate) fn repo(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("kepler-orbit-wt-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q"]);
        std::fs::write(d.join("a.txt"), "one\n").unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", "init"]);
        d
    }

    #[test]
    fn worktrees_isolate_changes_and_merge_cleanly_or_report_conflicts() {
        let r = repo("a");
        let base = resolve_base(&r, "HEAD").unwrap();
        assert_eq!(base.len(), 40);
        assert!(resolve_base(&r, "--output=x").is_err());
        assert!(resolve_base(&r, "no-such-branch").is_err());

        let w1 = r
            .parent()
            .unwrap()
            .join(format!("kepler-orbit-wt-{}-a-w1", std::process::id()));
        let w2 = r
            .parent()
            .unwrap()
            .join(format!("kepler-orbit-wt-{}-a-w2", std::process::id()));
        add_worktree(&r, &w1, "kepler/orbit/t/one", &base).unwrap();
        add_worktree(&r, &w2, "kepler/orbit/t/two", &base).unwrap();
        std::fs::write(w1.join("a.txt"), "one\nfrom-1\n").unwrap();
        std::fs::write(w1.join("new1.txt"), "x\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(r.join("a.txt")).unwrap(),
            "one\n",
            "main working copy untouched"
        );
        assert_eq!(
            std::fs::read_to_string(w2.join("a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "one\n",
            "siblings are isolated"
        );
        let mut files = changed_files(&w1);
        files.sort();
        assert_eq!(files, vec!["a.txt", "new1.txt"]);
        assert!(commit_all(&w1, "orbit: one").unwrap());
        assert!(
            !commit_all(&w1, "orbit: again").unwrap(),
            "nothing left to commit"
        );

        // Non-conflicting merge brings the dependency's work in.
        merge_branches(&w2, &["kepler/orbit/t/one".into()]).unwrap();
        assert!(w2.join("new1.txt").exists());

        // Conflict: both change the same line differently.
        let w3 = r
            .parent()
            .unwrap()
            .join(format!("kepler-orbit-wt-{}-a-w3", std::process::id()));
        add_worktree(&r, &w3, "kepler/orbit/t/three", &base).unwrap();
        std::fs::write(w3.join("a.txt"), "one\nfrom-3\n").unwrap();
        commit_all(&w3, "orbit: three").unwrap();
        let w4 = r
            .parent()
            .unwrap()
            .join(format!("kepler-orbit-wt-{}-a-w4", std::process::id()));
        add_worktree(&r, &w4, "kepler/orbit/t/four", &base).unwrap();
        merge_branches(&w4, &["kepler/orbit/t/one".into()]).unwrap();
        let e = merge_branches(&w4, &["kepler/orbit/t/three".into()]).unwrap_err();
        assert!(e.starts_with("merge_conflict"), "{e}");
        assert!(
            changed_files(&w4).is_empty(),
            "merge was aborted, worktree is clean"
        );

        for (w, b) in [(&w1, "one"), (&w2, "two"), (&w3, "three"), (&w4, "four")] {
            assert!(remove_worktree(&r, w, &format!("kepler/orbit/t/{b}")));
            assert!(!w.exists());
        }
    }
}
