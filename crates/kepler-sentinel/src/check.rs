//! Project checks (ADR-0022): run the project's own lint/test commands, bounded, inside a
//! working tree, and turn their output into findings.
//!
//! * Commands are **argv lists, never a shell string**, and are named by the user (flags) or
//!   by an explicit `--detect` request. Kepler never runs a command because a file in the
//!   repository asks for it.
//! * Every command passes the Guardian preflight first; a `deny` is never run, a
//!   `require_approval` only with the caller's explicit consent.
//! * Wall-clock timeout with process-tree kill, capped output, no stdin, no color.
//! * Output is parsed into findings (rustc/cargo, `path:line:col: error`, tsc, pytest/jest
//!   failures); an unparsable failure still yields one finding with the exit status.

use crate::rules::Hit;
use kepler_events::ToolCallRequested;
use kepler_policy::{Decision, Guardian};
use kepler_storage::{Confidence, Severity};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
pub const MAX_CHECK_HITS: usize = 200;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSpec {
    pub name: String,
    pub argv: Vec<String>,
}

impl CheckSpec {
    /// `name=program arg "quoted arg"`; without `name=` the program name is used.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (name, cmd) = match s.split_once('=') {
            Some((n, c))
                if !n.is_empty()
                    && n.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
            {
                (Some(n.to_string()), c)
            }
            _ => (None, s),
        };
        let argv = split_args(cmd)?;
        let Some(first) = argv.first() else {
            return Err("empty command".into());
        };
        let name = name.unwrap_or_else(|| first.clone());
        Ok(Self { name, argv })
    }

    pub fn command_line(&self) -> String {
        self.argv.join(" ")
    }
}

fn split_args(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            (None, c) => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote".into());
    }
    if in_word {
        out.push(cur);
    }
    Ok(out)
}

/// Default checks for the project types found in `repo` (only on explicit request).
pub fn detect(repo: &Path) -> Vec<CheckSpec> {
    let mut v = Vec::new();
    let mk = |n: &str, c: &str| CheckSpec::parse(&format!("{n}={c}"));
    let push = |v: &mut Vec<CheckSpec>, r: Result<CheckSpec, String>| {
        if let Ok(s) = r {
            v.push(s);
        }
    };
    if repo.join("Cargo.toml").is_file() {
        push(&mut v, mk("cargo-clippy", "cargo clippy --all-targets"));
        push(&mut v, mk("cargo-test", "cargo test"));
    }
    if let Ok(pkg) = std::fs::read_to_string(repo.join("package.json")) {
        for script in ["lint", "typecheck", "test"] {
            let key = format!("\"{script}\"");
            if let Some(i) = pkg.find(&key) {
                let rest = &pkg[i + key.len()..];
                let value = rest.trim_start().trim_start_matches(':').trim_start();
                let placeholder = value.starts_with("\"echo \\\"Error: no test");
                if value.starts_with('"') && !placeholder {
                    push(
                        &mut v,
                        mk(&format!("npm-{script}"), &format!("npm run {script}")),
                    );
                }
            }
        }
    }
    if repo.join("pytest.ini").is_file()
        || repo.join("pyproject.toml").is_file() && repo.join("tests").is_dir()
    {
        push(&mut v, mk("pytest", "python -m pytest -q"));
    }
    if repo.join("go.mod").is_file() {
        push(&mut v, mk("go-vet", "go vet ./..."));
        push(&mut v, mk("go-test", "go test ./..."));
    }
    v
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Passed,
    Failed(i32),
    TimedOut,
    /// Not run: Guardian denied it, it needs approval, or the program could not be started.
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub command: String,
    pub status: CheckStatus,
    pub duration_ms: u64,
    pub output_truncated: bool,
    pub hits: Vec<Hit>,
}

/// Resolve `program` like the OS would, including `.cmd`/`.bat` shims on Windows.
fn resolve_program(program: &str, repo: &Path) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 {
        let full = if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo.join(p)
        };
        return full.is_file().then_some(full);
    }
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .map(str::to_lowercase)
            .collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        // The repository directory is never searched (a repo-local `cargo.exe` must not run).
        if dir.as_os_str().is_empty() || dir == Path::new(".") || dir == repo {
            continue;
        }
        for ext in std::iter::once(&String::new()).chain(exts.iter()) {
            let cand = dir.join(format!("{program}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Run one check. `approve` allows commands the Guardian marks `require_approval`.
pub fn run_check(
    repo: &Path,
    spec: &CheckSpec,
    timeout: Duration,
    guardian: &Guardian,
    approve: bool,
) -> CheckResult {
    let command = spec.command_line();
    let result = |status, duration_ms, truncated, hits| CheckResult {
        name: spec.name.clone(),
        command: command.clone(),
        status,
        duration_ms,
        output_truncated: truncated,
        hits,
    };
    let call = ToolCallRequested {
        tool: "check".into(),
        command: Some(command.clone()),
        paths: Vec::new(),
        destructive: false,
    };
    match guardian.preflight(&call) {
        Decision::Deny { reason } => {
            return result(
                CheckStatus::Skipped(format!("denied by Guardian: {reason}")),
                0,
                false,
                vec![],
            );
        }
        Decision::RequireApproval { reason } if !approve => {
            return result(
                CheckStatus::Skipped(format!("needs approval ({reason}); pass --yes to allow")),
                0,
                false,
                vec![],
            );
        }
        _ => {}
    }
    let Some(program) = spec.argv.first().and_then(|p| resolve_program(p, repo)) else {
        return result(
            CheckStatus::Skipped("program not found on PATH".into()),
            0,
            false,
            vec![],
        );
    };
    let started = Instant::now();
    let mut child = match Command::new(&program)
        .args(&spec.argv[1..])
        .current_dir(repo)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return result(
                CheckStatus::Skipped(format!("cannot start: {e}")),
                0,
                false,
                vec![],
            );
        }
    };
    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut s| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let mut over = false;
            loop {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if buf.len() < MAX_OUTPUT_BYTES {
                            let take = n.min(MAX_OUTPUT_BYTES - buf.len());
                            buf.extend_from_slice(&chunk[..take]);
                            over |= take < n;
                        } else {
                            over = true;
                        }
                    }
                }
            }
            let _ = tx.send((over, buf));
        })
    })
    .collect();
    drop(tx);
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    timed_out = true;
                    kill_tree(&mut child);
                    break -1;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break -1,
        }
    };
    // Grandchildren holding the pipes open must not hang us: give readers a short grace.
    let mut text = String::new();
    let mut truncated = false;
    let deadline = Instant::now() + Duration::from_secs(2);
    for _ in &readers {
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok((over, buf)) = rx.recv_timeout(left) {
            truncated |= over;
            text.push_str(&String::from_utf8_lossy(&buf));
            text.push('\n');
        }
    }
    let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if timed_out {
        let hit = Hit {
            rule_id: "check.timeout",
            severity: Severity::Medium,
            confidence: Confidence::Confirmed,
            path: spec.name.clone(),
            line: None,
            message: format!(
                "`{command}` did not finish within {}s and was killed",
                timeout.as_secs()
            ),
            evidence: spec.name.clone(),
        };
        return result(CheckStatus::TimedOut, ms, truncated, vec![hit]);
    }
    let mut hits = parse_output(&spec.name, &text, repo);
    let status = if exit == 0 {
        CheckStatus::Passed
    } else {
        if hits.is_empty() {
            let last = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            hits.push(Hit {
                rule_id: "check.failed",
                severity: Severity::Medium,
                confidence: Confidence::Confirmed,
                path: spec.name.clone(),
                line: None,
                message: format!("`{command}` exited with status {exit}"),
                evidence: last.chars().take(120).collect(),
            });
        }
        CheckStatus::Failed(exit)
    };
    result(status, ms, truncated, hits)
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn rel(path: &str, repo: &Path) -> String {
    let p = path.trim().trim_matches('"').replace('\\', "/");
    let r = repo.to_string_lossy().replace('\\', "/");
    let r = r.strip_prefix("//?/").unwrap_or(&r);
    p.strip_prefix(&format!("{r}/")).unwrap_or(&p).to_string()
}

fn mk_hit(
    rule_id: &'static str,
    severity: Severity,
    tool: &str,
    path: &str,
    line: Option<u32>,
    message: &str,
    repo: &Path,
) -> Option<Hit> {
    let path = rel(path, repo);
    if kepler_policy::sensitive::is_sensitive_path(&path) {
        return None;
    }
    Some(Hit {
        rule_id,
        severity,
        confidence: Confidence::Confirmed,
        path,
        line,
        message: format!("{tool}: {}", message.chars().take(160).collect::<String>()),
        evidence: message.chars().take(120).collect(),
    })
}

/// `path:line` / `path:line:col` from the tail of `s`.
fn split_loc(s: &str) -> Option<(String, u32)> {
    let parts: Vec<&str> = s.rsplitn(3, ':').collect();
    // rsplitn yields from the right: [col|line, line|path, path?]
    let (path, line) = match parts.as_slice() {
        [col, line, path]
            if col.trim().parse::<u32>().is_ok() && line.trim().parse::<u32>().is_ok() =>
        {
            (*path, line.trim().parse().ok()?)
        }
        [line, path, _] | [line, path] if line.trim().parse::<u32>().is_ok() => {
            (*path, line.trim().parse().ok()?)
        }
        _ => return None,
    };
    (!path.trim().is_empty()).then(|| (path.trim().to_string(), line))
}

/// Turn tool output into findings. Pure; exposed for tests.
pub fn parse_output(tool: &str, text: &str, repo: &Path) -> Vec<Hit> {
    let mut hits = Vec::new();
    let mut pending: Option<(Severity, &'static str, String)> = None;
    let lines: Vec<&str> = text.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        if hits.len() >= MAX_CHECK_HITS {
            break;
        }
        let line = raw.trim_end();
        let t = line.trim_start();
        // rustc / cargo: header then ` --> path:line:col`
        if let Some(rest) = t
            .strip_prefix("error")
            .or_else(|| t.strip_prefix("warning"))
        {
            let is_err = t.starts_with("error");
            let after = rest
                .trim_start_matches(|c: char| c == '[' || c.is_ascii_alphanumeric() || c == ']');
            if let Some(msg) = after.strip_prefix(':') {
                let msg = msg.trim();
                let noise = msg.starts_with("aborting due to")
                    || msg.starts_with("could not compile")
                    || msg.contains("warning emitted")
                    || msg.contains("warnings emitted")
                    || msg.starts_with("build failed")
                    || msg.starts_with("test failed");
                if !msg.is_empty()
                    && !noise
                    && lines[i + 1..]
                        .iter()
                        .take(3)
                        .any(|l| l.trim_start().starts_with("--> "))
                {
                    pending = Some((
                        if is_err {
                            Severity::Medium
                        } else {
                            Severity::Low
                        },
                        if is_err {
                            "check.error"
                        } else {
                            "check.warning"
                        },
                        msg.to_string(),
                    ));
                    continue;
                }
            }
        }
        if let Some(loc) = t.strip_prefix("--> ") {
            if let (Some((sev, rule, msg)), Some((path, ln))) = (pending.take(), split_loc(loc)) {
                hits.extend(mk_hit(rule, sev, tool, &path, Some(ln), &msg, repo));
            }
            continue;
        }
        // gcc/clang/eslint-unix/mypy: path:line[:col]: error|warning: msg
        for (needle, sev, rule) in [
            (": error", Severity::Medium, "check.error"),
            (": warning", Severity::Low, "check.warning"),
        ] {
            if let Some(i) = line.find(needle) {
                if let Some((path, ln)) = split_loc(&line[..i]) {
                    if !path.contains(' ') || path.contains('/') || path.contains('\\') {
                        let msg = line[i + needle.len()..].trim_start_matches(':').trim();
                        hits.extend(mk_hit(rule, sev, tool, &path, Some(ln), msg, repo));
                        break;
                    }
                }
            }
        }
        // tsc: path(line,col): error TSxxxx: msg
        if let Some(open) = line.find('(') {
            if let Some(close) = line[open..].find("): error") {
                let path = &line[..open];
                let nums = &line[open + 1..open + close];
                if let Some(ln) = nums
                    .split(',')
                    .next()
                    .and_then(|n| n.trim().parse::<u32>().ok())
                {
                    let msg = line[open + close + 3..].trim();
                    hits.extend(mk_hit(
                        "check.error",
                        Severity::Medium,
                        tool,
                        path,
                        Some(ln),
                        msg,
                        repo,
                    ));
                    continue;
                }
            }
        }
        // pytest: FAILED path::test - reason ; jest/vitest: FAIL path
        if let Some(rest) = t.strip_prefix("FAILED ") {
            let id = rest.split(" - ").next().unwrap_or(rest);
            let path = id.split("::").next().unwrap_or(id);
            hits.extend(mk_hit(
                "check.test_failure",
                Severity::High,
                tool,
                path,
                None,
                &format!("test failed: {id}"),
                repo,
            ));
        } else if let Some(rest) = t.strip_prefix("FAIL ") {
            let path = rest.split_whitespace().next().unwrap_or("");
            if !path.is_empty() {
                hits.extend(mk_hit(
                    "check.test_failure",
                    Severity::High,
                    tool,
                    path,
                    None,
                    &format!("test file failed: {path}"),
                    repo,
                ));
            }
        } else if let Some(rest) = t.strip_prefix("test ") {
            // cargo test: "test module::name ... FAILED"
            if let Some(name) = rest.strip_suffix(" ... FAILED") {
                hits.extend(mk_hit(
                    "check.test_failure",
                    Severity::High,
                    tool,
                    tool,
                    None,
                    &format!("test failed: {name}"),
                    repo,
                ));
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_parsing() {
        let s = CheckSpec::parse("lint=cargo clippy --all-targets").unwrap();
        assert_eq!((s.name.as_str(), s.argv.len()), ("lint", 3));
        let s = CheckSpec::parse("npm run \"my script\"").unwrap();
        assert_eq!(s.name, "npm");
        assert_eq!(s.argv[2], "my script");
        assert!(CheckSpec::parse("").is_err());
        assert!(CheckSpec::parse("x=\"open").is_err());
    }

    #[test]
    fn parses_rustc_gcc_tsc_pytest_and_cargo_test() {
        let repo = Path::new("C:/repo");
        let out = "error[E0425]: cannot find value `x` in this scope\n  --> src/lib.rs:12:5\n   |\nwarning: unused variable: `y`\n --> C:/repo/src/main.rs:3:9\n\nerror: could not compile `foo` (lib) due to 1 previous error\nsrc/a.c:7:3: error: expected ';'\nweb/app.ts(14,2): error TS2322: Type 'string'\nFAILED tests/test_x.py::test_y - AssertionError\ntest parser::tests::bad ... FAILED\n";
        let hits = parse_output("t", out, repo);
        let key: Vec<_> = hits
            .iter()
            .map(|h| (h.rule_id, h.path.as_str(), h.line))
            .collect();
        assert!(
            key.contains(&("check.error", "src/lib.rs", Some(12))),
            "{key:?}"
        );
        assert!(
            key.contains(&("check.warning", "src/main.rs", Some(3))),
            "{key:?}"
        );
        assert!(
            key.contains(&("check.error", "src/a.c", Some(7))),
            "{key:?}"
        );
        assert!(
            key.contains(&("check.error", "web/app.ts", Some(14))),
            "{key:?}"
        );
        assert!(
            key.contains(&("check.test_failure", "tests/test_x.py", None)),
            "{key:?}"
        );
        assert_eq!(
            hits.iter()
                .filter(|h| h.message.contains("could not compile"))
                .count(),
            0
        );
        assert!(hits
            .iter()
            .any(|h| h.message.contains("parser::tests::bad")));
    }

    #[test]
    fn sensitive_paths_in_output_are_not_recorded() {
        let hits = parse_output("t", "x/.env:3:1: error: bad value\n", Path::new("."));
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_reads_manifests_only() {
        let d = std::env::temp_dir().join(format!("kepler-detect-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        std::fs::write(
            d.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        let names: Vec<_> = detect(&d).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["cargo-clippy", "cargo-test", "npm-lint"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn guardian_gates_commands_and_failures_become_findings() {
        let d = std::env::temp_dir().join(format!("kepler-check-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let g = Guardian::default();
        // denied: reads a secret
        let r = run_check(
            &d,
            &CheckSpec::parse("x=cat .env").unwrap(),
            Duration::from_secs(5),
            &g,
            true,
        );
        assert!(
            matches!(r.status, CheckStatus::Skipped(ref s) if s.contains("denied")),
            "{:?}",
            r.status
        );
        // needs approval
        let r = run_check(
            &d,
            &CheckSpec::parse("x=git reset --hard").unwrap(),
            Duration::from_secs(5),
            &g,
            false,
        );
        assert!(matches!(r.status, CheckStatus::Skipped(ref s) if s.contains("approval")));
        // missing program
        let r = run_check(
            &d,
            &CheckSpec::parse("nope-kepler-xyz").unwrap(),
            Duration::from_secs(5),
            &g,
            false,
        );
        assert!(matches!(r.status, CheckStatus::Skipped(_)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
