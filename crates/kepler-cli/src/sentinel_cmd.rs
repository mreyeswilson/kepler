//! `kepler sentinel ...`: deterministic change audit. `audit` reads `git diff` only (never runs
//! project code); findings are stored only with `--store`.

use clap::Subcommand;
use kepler_sentinel::{audit_repo, AuditScope, Hit};
use kepler_storage::{
    dismiss_finding, store_findings, FindingFilter, FindingRow, Reader, Severity, Writer,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Subcommand)]
pub enum SentinelCmd {
    /// Audit uncommitted changes (staged + unstaged + small untracked text files) of a repository.
    Audit {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Only staged changes.
        #[arg(long, conflicts_with = "base")]
        staged: bool,
        /// Compare the working tree with this revision instead of HEAD.
        #[arg(long)]
        base: Option<String>,
        /// Project id used when storing findings (default: derived from the repository path).
        #[arg(long)]
        project: Option<String>,
        /// Persist findings to the local database (redacted, bounded).
        #[arg(long)]
        store: bool,
        #[arg(long)]
        json: bool,
        /// Exit with status 1 when a finding of at least this severity exists.
        #[arg(long, value_parser = parse_severity)]
        fail_on: Option<Severity>,
    },
    /// Run the project's own lint/test commands (bounded, Guardian-gated) and record findings.
    /// Commands come only from `--cmd` or an explicit `--detect`, never from repository files.
    Check {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// `name=program args...` (argv, no shell). Repeatable.
        #[arg(long = "cmd")]
        cmds: Vec<String>,
        /// Use defaults for the manifests found (Cargo.toml, package.json scripts, pytest, go.mod).
        #[arg(long)]
        detect: bool,
        /// Per-command wall-clock limit.
        #[arg(long, default_value_t = 300)]
        timeout_secs: u64,
        /// Allow commands the Guardian classifies as needing approval (denied ones never run).
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        store: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, value_parser = parse_severity)]
        fail_on: Option<Severity>,
    },
    /// List stored findings (newest first).
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_parser = parse_severity)]
        min_severity: Option<Severity>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        /// Include dismissed findings.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Hide a finding from default listings (it stays in the database).
    Dismiss { id: i64 },
}

fn parse_severity(s: &str) -> Result<Severity, String> {
    Severity::parse(s).ok_or_else(|| "expected info|low|medium|high|critical".to_string())
}

pub(crate) fn project_for(repo: &std::path::Path) -> String {
    let canon = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canon.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("local-{h:016x}")
}

fn hit_json(h: &Hit) -> Value {
    json!({
        "rule": h.rule_id, "severity": h.severity.as_str(), "confidence": h.confidence.as_str(),
        "path": h.path, "line": h.line, "message": h.message, "evidence": h.evidence,
    })
}

fn row_json(r: &FindingRow) -> Value {
    json!({
        "id": r.id, "at": r.ts, "project": r.project_id, "source": r.source, "rule": r.rule_id,
        "severity": r.severity, "confidence": r.confidence, "path": r.path, "line": r.line,
        "message": r.message, "evidence": r.evidence, "status": r.status,
    })
}

fn loc(path: &str, line: Option<impl std::fmt::Display>) -> String {
    match line {
        Some(l) => format!("{path}:{l}"),
        None => path.to_string(),
    }
}

pub fn run(cmd: SentinelCmd) -> ExitCode {
    match exec(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kepler: {e}");
            ExitCode::FAILURE
        }
    }
}

fn exec(cmd: SentinelCmd) -> Result<ExitCode, String> {
    match cmd {
        SentinelCmd::Audit {
            repo,
            staged,
            base,
            project,
            store,
            json,
            fail_on,
        } => {
            let scope = match (staged, base) {
                (true, _) => AuditScope::Staged,
                (false, Some(b)) => AuditScope::Base(b),
                _ => AuditScope::Worktree,
            };
            let report = audit_repo(&repo, &scope).map_err(|e| e.to_string())?;
            let project = project.unwrap_or_else(|| project_for(&repo));
            let mut stored = None;
            if store && !report.hits.is_empty() {
                let path = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
                let writer = Writer::start(&path, Default::default()).map_err(|e| e.to_string())?;
                let findings: Vec<_> = report
                    .hits
                    .iter()
                    .cloned()
                    .map(|h| h.into_finding(&project, ""))
                    .collect();
                stored = Some(
                    writer
                        .exec(Duration::from_secs(5), move |c| {
                            store_findings(c, &findings)
                        })
                        .map_err(|e| e.to_string())?,
                );
            }
            if json {
                println!(
                    "{}",
                    json!({
                        "files_analyzed": report.files, "untracked_scanned": report.untracked_scanned,
                        "truncated": report.truncated, "project": project, "stored_new": stored,
                        "findings": report.hits.iter().map(hit_json).collect::<Vec<_>>(),
                    })
                );
            } else {
                println!(
                    "sentinel: {} files analyzed, {} findings{}",
                    report.files,
                    report.hits.len(),
                    if report.truncated {
                        " (diff truncated at the size cap)"
                    } else {
                        ""
                    }
                );
                let mut hits = report.hits.clone();
                hits.sort_by_key(|h| std::cmp::Reverse(h.severity));
                for h in &hits {
                    println!(
                        "  [{:8} {:9}] {} {}\n      {}",
                        h.severity.as_str(),
                        h.confidence.as_str(),
                        loc(&h.path, h.line),
                        h.rule_id,
                        h.message
                    );
                }
                match stored {
                    Some(n) => println!("stored {n} new finding(s) for project {project}"),
                    None if store => {}
                    None => println!("(not stored; pass --store to keep these findings)"),
                }
            }
            let failed = fail_on.is_some_and(|min| report.hits.iter().any(|h| h.severity >= min));
            Ok(if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        SentinelCmd::Check {
            repo,
            cmds,
            detect,
            timeout_secs,
            yes,
            project,
            store,
            json,
            fail_on,
        } => {
            use kepler_sentinel::{CheckSpec, CheckStatus};
            let mut specs = Vec::new();
            for c in &cmds {
                specs.push(CheckSpec::parse(c).map_err(|e| format!("--cmd {c:?}: {e}"))?);
            }
            if detect {
                specs.extend(kepler_sentinel::detect(&repo));
            }
            if specs.is_empty() {
                return Err("nothing to run: pass --cmd \"name=program args\" or --detect".into());
            }
            let (rules, _) = kepler_core::pipeline::load_guardian_rules();
            let guardian = kepler_policy::Guardian::new(rules);
            let project = project.unwrap_or_else(|| project_for(&repo));
            let mut results = Vec::new();
            for s in &specs {
                results.push(kepler_sentinel::run_check(
                    &repo,
                    s,
                    Duration::from_secs(timeout_secs.max(1)),
                    &guardian,
                    yes,
                ));
            }
            let hits: Vec<Hit> = results.iter().flat_map(|r| r.hits.clone()).collect();
            let mut stored = None;
            if store && !hits.is_empty() {
                let path = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
                let writer = Writer::start(&path, Default::default()).map_err(|e| e.to_string())?;
                let findings: Vec<_> = hits
                    .iter()
                    .cloned()
                    .map(|h| h.into_finding(&project, ""))
                    .collect();
                stored = Some(
                    writer
                        .exec(Duration::from_secs(5), move |c| {
                            store_findings(c, &findings)
                        })
                        .map_err(|e| e.to_string())?,
                );
            }
            let status_str = |s: &CheckStatus| match s {
                CheckStatus::Passed => "passed".to_string(),
                CheckStatus::Failed(c) => format!("failed({c})"),
                CheckStatus::TimedOut => "timed_out".to_string(),
                CheckStatus::Skipped(r) => format!("skipped: {r}"),
            };
            if json {
                println!(
                    "{}",
                    json!({
                        "project": project, "stored_new": stored,
                        "checks": results.iter().map(|r| json!({
                            "name": r.name, "command": r.command, "status": status_str(&r.status),
                            "duration_ms": r.duration_ms, "output_truncated": r.output_truncated,
                            "findings": r.hits.len(),
                        })).collect::<Vec<_>>(),
                        "findings": hits.iter().map(hit_json).collect::<Vec<_>>(),
                    })
                );
            } else {
                for r in &results {
                    println!(
                        "check {:14} {:10} {:>6} ms  {} finding(s)  `{}`",
                        r.name,
                        status_str(&r.status),
                        r.duration_ms,
                        r.hits.len(),
                        r.command
                    );
                }
                let mut sorted = hits.clone();
                sorted.sort_by_key(|h| std::cmp::Reverse(h.severity));
                for h in sorted.iter().take(30) {
                    println!(
                        "  [{:8}] {} {}\n      {}",
                        h.severity.as_str(),
                        loc(&h.path, h.line),
                        h.rule_id,
                        h.message
                    );
                }
                match stored {
                    Some(n) => println!("stored {n} new finding(s) for project {project}"),
                    None if store => {}
                    None => println!("(not stored; pass --store to keep these findings)"),
                }
            }
            let any_failed = results
                .iter()
                .any(|r| matches!(r.status, CheckStatus::Failed(_) | CheckStatus::TimedOut));
            let sev_fail = fail_on.is_some_and(|min| hits.iter().any(|h| h.severity >= min));
            Ok(if any_failed || sev_fail {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        SentinelCmd::List {
            project,
            min_severity,
            limit,
            all,
            json,
        } => {
            let path = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
            let rows = if path.exists() {
                Reader::open(&path)
                    .and_then(|r| {
                        r.findings(&FindingFilter {
                            project_id: project,
                            min_severity,
                            include_dismissed: all,
                            limit,
                        })
                    })
                    .map_err(|e| e.to_string())?
            } else {
                Vec::new()
            };
            if json {
                println!("{}", Value::Array(rows.iter().map(row_json).collect()));
            } else if rows.is_empty() {
                println!("no findings");
            } else {
                for r in &rows {
                    println!(
                        "#{} [{:8} {:9}] {} {} ({}, {})\n    {}",
                        r.id,
                        r.severity,
                        r.confidence,
                        loc(&r.path, r.line),
                        r.rule_id,
                        r.source,
                        r.status,
                        r.message
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        SentinelCmd::Dismiss { id } => {
            let path = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
            if !path.exists() {
                return Err("no database yet".into());
            }
            let writer = Writer::start(&path, Default::default()).map_err(|e| e.to_string())?;
            let done = writer
                .exec(Duration::from_secs(5), move |c| dismiss_finding(c, id))
                .map_err(|e| e.to_string())?;
            println!(
                "{}",
                if done {
                    "dismissed"
                } else {
                    "no open finding with that id"
                }
            );
            Ok(if done {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}
