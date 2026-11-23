//! `kepler vision ...`: on-demand UI inspection through the separate Playwright worker. The worker
//! is started only by `check` (or `status --probe`), for the duration of the command, and only
//! for URLs on the allowlist (localhost by default).

use clap::Subcommand;
use kepler_storage::{store_findings, Writer};
use kepler_vision::{
    parse_allowed_hosts, to_findings, CheckOptions, Viewport, VisionError, WorkerCommand,
    WorkerManager,
};
use serde_json::json;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Subcommand)]
pub enum VisionCmd {
    /// Open a URL in a headless browser and report overflow, console errors and basic a11y issues.
    Check {
        url: String,
        /// WIDTHxHEIGHT (repeatable, max 4). Default: 375x812, 768x1024, 1280x800.
        #[arg(long = "viewport", value_parser = parse_viewport)]
        viewports: Vec<Viewport>,
        #[arg(long, default_value_t = 15)]
        timeout_secs: u64,
        /// Save screenshots under the Kepler data directory (opt-in; never stored in the database).
        #[arg(long)]
        screenshot: bool,
        /// Persist findings to the local database (source `vision`).
        #[arg(long)]
        store: bool,
        #[arg(long, default_value = "local")]
        project: String,
        #[arg(long)]
        json: bool,
    },
    /// Show how the worker is configured; `--probe` starts it once to check that it answers.
    Status {
        #[arg(long)]
        probe: bool,
    },
    /// Delete saved screenshots.
    Clean,
}

fn parse_viewport(s: &str) -> Result<Viewport, String> {
    Viewport::parse(s)
}

fn allowed_hosts() -> Vec<String> {
    parse_allowed_hosts(std::env::var("KEPLER_VISION_ALLOW_HOSTS").ok().as_deref())
}

fn vision_dir() -> Result<std::path::PathBuf, String> {
    Ok(kepler_storage::dirs::data_dir()
        .map_err(|e| e.to_string())?
        .join("vision"))
}

pub fn run(cmd: VisionCmd) -> ExitCode {
    match exec(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kepler: {e}");
            // Refused/unconfigured is a usage problem, not a crash.
            ExitCode::from(2)
        }
    }
}

fn exec(cmd: VisionCmd) -> Result<ExitCode, VisionOrString> {
    match cmd {
        VisionCmd::Status { probe } => {
            let hosts = allowed_hosts();
            let cmd = WorkerCommand::from_env();
            println!("allowlist: localhost, 127.0.0.1, [::1], *.localhost (+ {hosts:?})");
            match cmd {
                Err(e) => {
                    println!("worker: not configured ({e})");
                    Ok(ExitCode::SUCCESS)
                }
                Ok(c) => {
                    println!("worker: {:?} {:?}", c.program, c.args);
                    if !probe {
                        println!("(not started; pass --probe to start it once)");
                        return Ok(ExitCode::SUCCESS);
                    }
                    let mut m = WorkerManager::new(c, hosts);
                    m.probe()?;
                    println!("worker answered hello; Playwright package is importable");
                    m.shutdown();
                    Ok(ExitCode::SUCCESS)
                }
            }
        }
        VisionCmd::Clean => {
            let dir = vision_dir()?;
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
                println!("removed {}", dir.display());
            } else {
                println!("nothing to remove");
            }
            Ok(ExitCode::SUCCESS)
        }
        VisionCmd::Check {
            url,
            viewports,
            timeout_secs,
            screenshot,
            store,
            project,
            json,
        } => {
            let viewports = if viewports.is_empty() {
                Viewport::defaults()
            } else {
                viewports
            };
            let screenshot_dir = if screenshot {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                Some(vision_dir()?.join(ms.to_string()))
            } else {
                None
            };
            let mut mgr = WorkerManager::new(WorkerCommand::from_env()?, allowed_hosts());
            let result = mgr.check(
                &url,
                &viewports,
                &CheckOptions {
                    timeout: Duration::from_secs(timeout_secs),
                    screenshot_dir,
                },
            );
            mgr.shutdown();
            let result = result?;
            let findings = to_findings(&project, "", &result);
            let mut stored = None;
            if store && !findings.is_empty() {
                let path = kepler_storage::dirs::db_path().map_err(|e| e.to_string())?;
                let writer = Writer::start(&path, Default::default()).map_err(|e| e.to_string())?;
                let f = findings.clone();
                stored = Some(
                    writer
                        .exec(Duration::from_secs(5), move |c| store_findings(c, &f))
                        .map_err(|e| e.to_string())?,
                );
            }
            if json {
                let vps: Vec<_> = result
                    .viewports
                    .iter()
                    .map(|v| {
                        json!({
                            "name": v.name, "width": v.width, "height": v.height,
                            "horizontal_overflow": v.horizontal_overflow,
                            "scroll_width": v.scroll_width, "client_width": v.client_width,
                            "console_errors": v.console_errors, "page_errors": v.page_errors,
                            "failed_requests": v.failed_requests,
                            "issues": v.issues.iter().map(|i| json!({"rule": i.rule, "count": i.count, "samples": i.samples})).collect::<Vec<_>>(),
                            "screenshot": v.screenshot,
                        })
                    })
                    .collect();
                println!(
                    "{}",
                    json!({"url": result.url, "title": result.title, "viewports": vps,
                           "findings": findings.len(), "stored_new": stored})
                );
            } else {
                println!("vision: {} ({:?})", result.url, result.title);
                for v in &result.viewports {
                    println!(
                        "  {} {}x{}: overflow={} console_errors={} page_errors={} failed_requests={} issues={}",
                        v.name, v.width, v.height, v.horizontal_overflow, v.console_errors.len(),
                        v.page_errors.len(), v.failed_requests.len(), v.issues.len()
                    );
                    for i in &v.issues {
                        println!("      {} x{} {}", i.rule, i.count, i.samples.join(", "));
                    }
                    if let Some(s) = &v.screenshot {
                        println!("      screenshot: {s}");
                    }
                }
                match stored {
                    Some(n) => println!("stored {n} new finding(s)"),
                    None => println!("(findings not stored; pass --store)"),
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Error type that lets `?` accept both worker errors and plain strings.
#[derive(Debug)]
pub struct VisionOrString(String);

impl std::fmt::Display for VisionOrString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for VisionOrString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<VisionError> for VisionOrString {
    fn from(e: VisionError) -> Self {
        Self(e.to_string())
    }
}
