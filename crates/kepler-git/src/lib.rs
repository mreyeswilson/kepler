//! Bounded git invocations for Sentinel and Orbit: fixed argument vectors (never a shell),
//! a wall-clock timeout with kill, capped stdout/stderr, no terminal prompts.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct GitOptions {
    pub timeout: Duration,
    /// Stdout bytes kept; the rest is drained and discarded (`truncated` is set).
    pub max_stdout: usize,
}

impl Default for GitOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_stdout: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    pub stdout: String,
    pub truncated: bool,
}

#[derive(Debug)]
pub enum GitError {
    /// `git` could not be started (not installed / not on PATH).
    Spawn(String),
    Timeout,
    Failed {
        code: Option<i32>,
        stderr: String,
    },
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot run git: {e}"),
            Self::Timeout => write!(f, "git timed out"),
            Self::Failed { code, stderr } => {
                write!(f, "git failed (exit {code:?}): {}", stderr.trim())
            }
        }
    }
}

impl std::error::Error for GitError {}

/// A conservative revision/branch spelling: never starts with `-` (option injection), no
/// whitespace or shell/git metacharacters beyond those revisions legitimately use.
pub fn valid_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('-')
        && !s.contains("..")
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | '~' | '^' | '@')
        })
}

fn read_capped(mut r: impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
                if n > room {
                    truncated = true;
                }
            }
        }
    }
    (kept, truncated)
}

/// Run `git -C dir <args>`.
pub fn run(dir: &Path, args: &[&str], opts: &GitOptions) -> Result<GitOutput, GitError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off", "-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitError::Spawn(e.to_string()))?;
    let out = child.stdout.take();
    let err = child.stderr.take();
    let cap = opts.max_stdout;
    let t_out =
        std::thread::spawn(move || out.map_or((Vec::new(), false), |o| read_capped(o, cap)));
    let t_err =
        std::thread::spawn(move || err.map_or((Vec::new(), false), |e| read_capped(e, 4096)));
    let deadline = Instant::now() + opts.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(GitError::Spawn(e.to_string())),
        }
    };
    let (stdout, truncated) = t_out.join().unwrap_or_default();
    let (stderr, _) = t_err.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(GitError::Timeout);
    };
    if !status.success() {
        return Err(GitError::Failed {
            code: status.code(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        });
    }
    Ok(GitOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        truncated,
    })
}

/// Is `dir` inside a git work tree?
pub fn is_repo(dir: &Path) -> bool {
    run(
        dir,
        &["rev-parse", "--is-inside-work-tree"],
        &GitOptions::default(),
    )
    .is_ok_and(|o| o.stdout.trim() == "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_cannot_inject_options_or_metacharacters() {
        for ok in ["HEAD", "main", "feature/x-1", "HEAD~2", "v1.0.0", "abc123"] {
            assert!(valid_ref(ok), "{ok}");
        }
        for bad in [
            "",
            "-D",
            "--output=x",
            "a b",
            "a;b",
            "a..b",
            "$(x)",
            "a\nb",
            "a|b",
        ] {
            assert!(!valid_ref(bad), "{bad}");
        }
    }

    #[test]
    fn missing_directory_is_an_error_not_a_panic() {
        let r = run(
            Path::new("definitely/not/here"),
            &["status"],
            &GitOptions::default(),
        );
        assert!(r.is_err());
    }

    #[test]
    fn capped_reader_discards_the_excess() {
        let data = vec![b'x'; 20_000];
        let (kept, truncated) = read_capped(&data[..], 100);
        assert_eq!((kept.len(), truncated), (100, true));
    }
}
