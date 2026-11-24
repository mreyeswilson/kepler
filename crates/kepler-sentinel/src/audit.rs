//! Repository audit: `git diff` added lines (+ small untracked text files) through the rules.

use crate::diff::{parse_unified, FileDiff, MAX_FILES, MAX_LINE_CHARS};
use crate::rules::{audit_diffs, Hit};
use kepler_git::{run, valid_ref, GitError, GitOptions};
use kepler_policy::sensitive::is_sensitive_path;
use std::io::Read;
use std::path::Path;

const MAX_UNTRACKED: usize = 50;
const MAX_UNTRACKED_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditScope {
    /// Everything that differs from `HEAD` (staged and unstaged) plus untracked text files.
    Worktree,
    /// Staged changes only.
    Staged,
    /// Working tree against a revision (e.g. `main`, `HEAD~3`).
    Base(String),
}

#[derive(Debug)]
pub enum AuditError {
    NotARepo,
    BadRevision,
    Git(GitError),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotARepo => write!(f, "not inside a git work tree"),
            Self::BadRevision => write!(f, "invalid revision spelling"),
            Self::Git(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AuditError {}

impl From<GitError> for AuditError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditReport {
    pub files: usize,
    pub untracked_scanned: usize,
    /// The diff exceeded the output cap and only a prefix was analyzed.
    pub truncated: bool,
    pub hits: Vec<Hit>,
}

const DIFF_ARGS: [&str; 5] = [
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-renames",
    "--unified=0",
];

fn diff_text(root: &Path, scope: &AuditScope) -> Result<(String, bool), AuditError> {
    let opts = GitOptions::default();
    let args: Vec<&str> = DIFF_ARGS.to_vec();
    let mut cached = args.clone();
    cached.push("--cached");
    match scope {
        AuditScope::Staged => {
            let o = run(root, &cached, &opts)?;
            Ok((o.stdout, o.truncated))
        }
        AuditScope::Base(rev) => {
            if !valid_ref(rev) {
                return Err(AuditError::BadRevision);
            }
            let mut a = args;
            a.push(rev);
            let o = run(root, &a, &opts)?;
            Ok((o.stdout, o.truncated))
        }
        AuditScope::Worktree => {
            let mut with_head = args.clone();
            with_head.push("HEAD");
            match run(root, &with_head, &opts) {
                Ok(o) => Ok((o.stdout, o.truncated)),
                // No commits yet: staged plus unstaged.
                Err(GitError::Failed { .. }) => {
                    let a = run(root, &cached, &opts)?;
                    let b = run(root, &args, &opts)?;
                    Ok((
                        format!("{}\n{}", a.stdout, b.stdout),
                        a.truncated || b.truncated,
                    ))
                }
                Err(e) => Err(e.into()),
            }
        }
    }
}

fn untracked_files(root: &Path) -> Vec<FileDiff> {
    let Ok(o) = run(
        root,
        &["ls-files", "--others", "--exclude-standard"],
        &GitOptions::default(),
    ) else {
        return Vec::new();
    };
    let list: Vec<String> = o.stdout.lines().map(str::to_string).collect();
    untracked_from(root, &list)
}

fn untracked_from(root: &Path, list: &[String]) -> Vec<FileDiff> {
    let mut out = Vec::new();
    for rel in list.iter().take(MAX_UNTRACKED) {
        let rel = rel.as_str();
        if rel.is_empty() {
            continue;
        }
        if is_sensitive_path(rel) {
            // Reported by the path rule; the content is never read.
            out.push(FileDiff {
                path: rel.to_string(),
                ..FileDiff::default()
            });
            continue;
        }
        let full = root.join(rel);
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mut buf = Vec::new();
        let Ok(f) = std::fs::File::open(&full) else {
            continue;
        };
        if f.take(MAX_UNTRACKED_BYTES).read_to_end(&mut buf).is_err() || buf.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&buf);
        out.push(FileDiff {
            path: rel.to_string(),
            added: text
                .lines()
                .enumerate()
                .map(|(i, l)| {
                    (
                        u32::try_from(i + 1).unwrap_or(u32::MAX),
                        l.chars().take(MAX_LINE_CHARS).collect(),
                    )
                })
                .collect(),
            removed: Vec::new(),
        });
    }
    out
}

/// A repository-relative path safe to hand to git as a literal pathspec.
fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && !p.starts_with(['-', '/', '\\'])
        && !p.contains(['\0', '\n', '\r', ':'])
        && !p.split(['/', '\\']).any(|s| s == "..")
}

/// Audit only `paths` (relative to `root`) of the repository: `git diff HEAD -- <paths>` plus
/// those of them that are untracked, with the same rules as [`audit_repo`] (ADR-0027). Bounded,
/// read-only, never reads sensitive files' content. Unsafe paths are ignored.
pub fn audit_files(root: &Path, paths: &[String]) -> Result<AuditReport, AuditError> {
    if !kepler_git::is_repo(root) {
        return Err(AuditError::NotARepo);
    }
    let rel: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|p| safe_rel(p))
        .take(100)
        .collect();
    if rel.is_empty() {
        return Ok(AuditReport {
            files: 0,
            untracked_scanned: 0,
            truncated: false,
            hits: Vec::new(),
        });
    }
    let opts = GitOptions::default();
    let with = |base: &[&'static str]| -> Vec<&str> {
        let mut a: Vec<&str> = vec!["--literal-pathspecs"];
        a.extend_from_slice(base);
        a.push("--");
        a.extend(rel.iter().copied());
        a
    };
    let mut diff_args: Vec<&str> = DIFF_ARGS.to_vec();
    diff_args.push("HEAD");
    let (text, truncated) = match run(root, &with(&diff_args), &opts) {
        Ok(o) => (o.stdout, o.truncated),
        // No commits yet: staged plus unstaged.
        Err(GitError::Failed { .. }) => {
            let mut cached = DIFF_ARGS.to_vec();
            cached.push("--cached");
            let a = run(root, &with(&cached), &opts)?;
            let b = run(root, &with(&DIFF_ARGS), &opts)?;
            (format!("{}\n{}", a.stdout, b.stdout), a.truncated || b.truncated)
        }
        Err(e) => return Err(e.into()),
    };
    let mut files = parse_unified(&text);
    let untracked: Vec<String> = run(
        root,
        &with(&["ls-files", "--others", "--exclude-standard"]),
        &opts,
    )
    .map(|o| o.stdout.lines().map(str::to_string).collect())
    .unwrap_or_default();
    let extra = untracked_from(root, &untracked);
    let untracked_scanned = extra.iter().filter(|f| !f.added.is_empty()).count();
    files.extend(extra);
    files.truncate(MAX_FILES);
    Ok(AuditReport {
        files: files.len(),
        untracked_scanned,
        truncated,
        hits: audit_diffs(&files),
    })
}

/// Audit the repository containing `root`. Read-only: it runs `git diff`/`ls-files` only.
pub fn audit_repo(root: &Path, scope: &AuditScope) -> Result<AuditReport, AuditError> {
    if !kepler_git::is_repo(root) {
        return Err(AuditError::NotARepo);
    }
    let (text, truncated) = diff_text(root, scope)?;
    let mut files = parse_unified(&text);
    let mut untracked_scanned = 0;
    if *scope == AuditScope::Worktree {
        let extra = untracked_files(root);
        untracked_scanned = extra.iter().filter(|f| !f.added.is_empty()).count();
        files.extend(extra);
    }
    files.truncate(MAX_FILES);
    Ok(AuditReport {
        files: files.len(),
        untracked_scanned,
        truncated,
        hits: audit_diffs(&files),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let s = Command::new("git")
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
            s.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&s.stderr)
        );
    }

    fn repo(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("kepler-sentinel-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q"]);
        std::fs::write(d.join("a.rs"), "fn main() {}\n").unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", "init"]);
        d
    }

    #[test]
    fn finds_added_lines_with_file_and_line_and_untracked_files() {
        let d = repo("a");
        std::fs::write(
            d.join("a.rs"),
            format!(
                "fn main() {{}}\nlet t = \"ghp_{}\";\nlet x = y.unwrap();\n",
                "a1B2".repeat(9)
            ),
        )
        .unwrap();
        std::fs::write(d.join("new.js"), "eval(x)\n").unwrap();
        std::fs::write(d.join(".env"), "TOKEN=abc\n").unwrap();
        let r = audit_repo(&d, &AuditScope::Worktree).unwrap();
        let ids: Vec<(&str, &str, Option<u32>)> = r
            .hits
            .iter()
            .map(|h| (h.rule_id, h.path.as_str(), h.line))
            .collect();
        assert!(ids.contains(&("diff.secret", "a.rs", Some(2))), "{ids:?}");
        assert!(ids.contains(&("rust.unwrap", "a.rs", Some(3))), "{ids:?}");
        assert!(ids.contains(&("js.eval", "new.js", Some(1))), "{ids:?}");
        assert!(ids
            .iter()
            .any(|(r, p, _)| *r == "path.sensitive" && *p == ".env"));
        // The content of the sensitive untracked file was never analyzed.
        assert!(!ids
            .iter()
            .any(|(r, p, _)| *p == ".env" && *r == "diff.secret"));
        assert_eq!(r.untracked_scanned, 1);
        assert!(!r.truncated);
    }

    #[test]
    fn staged_scope_base_scope_and_clean_repo() {
        let d = repo("b");
        assert!(audit_repo(&d, &AuditScope::Worktree)
            .unwrap()
            .hits
            .is_empty());
        std::fs::write(d.join("a.rs"), "fn main() {}\nunsafe { z() }\n").unwrap();
        assert!(audit_repo(&d, &AuditScope::Staged).unwrap().hits.is_empty());
        git(&d, &["add", "a.rs"]);
        assert_eq!(audit_repo(&d, &AuditScope::Staged).unwrap().hits.len(), 1);
        git(&d, &["commit", "-q", "-m", "second"]);
        let r = audit_repo(&d, &AuditScope::Base("HEAD~1".into())).unwrap();
        assert!(r.hits.iter().any(|h| h.rule_id == "rust.unsafe"));
    }

    #[test]
    fn audit_files_only_looks_at_the_given_paths() {
        let d = repo("files");
        std::fs::write(d.join("a.rs"), "fn main() {}\nlet x = y.unwrap();\n").unwrap();
        std::fs::write(d.join("other.js"), "eval(x)\n").unwrap();
        std::fs::write(d.join("new.py"), "eval(a)\n").unwrap();
        let r = audit_files(&d, &["a.rs".into(), "new.py".into(), "../etc".into(), "-x".into()])
            .unwrap();
        let ids: Vec<(&str, &str)> = r.hits.iter().map(|h| (h.rule_id, h.path.as_str())).collect();
        assert!(ids.contains(&("rust.unwrap", "a.rs")), "{ids:?}");
        assert!(ids.contains(&("py.eval", "new.py")), "{ids:?}");
        assert!(!ids.iter().any(|(_, p)| *p == "other.js"), "{ids:?}");
        assert!(audit_files(&d, &["../x".into()]).unwrap().hits.is_empty());
    }

    #[test]
    fn rejects_bad_revisions() {
        let d = repo("c");
        assert!(matches!(
            audit_repo(&d, &AuditScope::Base("--output=x".into())),
            Err(AuditError::BadRevision)
        ));
    }

    #[test]
    fn repository_without_commits_is_audited() {
        let d = std::env::temp_dir().join(format!("kepler-sentinel-{}-fresh", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q"]);
        std::fs::write(d.join("x.py"), "eval(a)\n").unwrap();
        git(&d, &["add", "x.py"]);
        let r = audit_repo(&d, &AuditScope::Worktree).unwrap();
        assert!(
            r.hits.iter().any(|h| h.rule_id == "py.eval"),
            "{:?}",
            r.hits
        );
    }
}
