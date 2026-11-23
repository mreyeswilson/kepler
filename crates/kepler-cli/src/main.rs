//! `kepler` command line interface.

#[cfg(windows)]
mod detach;
mod hook_claude;
mod hook_cmd;
mod inspect;
mod mcp_cmd;
mod memory_cmd;
mod orbit_cmd;
mod sentinel_cmd;
mod ui_cmd;
mod vision_cmd;

use clap::{Parser, Subcommand};
use kepler_core::emit::emit_in_process;
use kepler_core::{
    doctor, emit_one_shot, Daemon, DaemonConfig, DaemonError, EmitOptions, EmitOutcome,
};
use kepler_events::{ENVELOPE_VERSION, MAX_ENVELOPE_BYTES};
use kepler_ipc::{Endpoint, PROTOCOL_VERSION};
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "kepler",
    about = "Kepler local-first agent middleware",
    disable_version_flag = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum McpCmd {
    /// Serve MCP over stdio (tools: kepler_doctor, kepler_inspect, kepler_memory_search).
    /// MCP gives the agent callable tools only; it does not intercept prompts or tool calls.
    Serve,
}

#[derive(Subcommand)]
enum Command {
    /// Read-only diagnostics: runtime, IPC endpoint, database, capabilities.
    Doctor {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Run the per-user daemon in the foreground (normally started lazily by `emit`).
    Daemon {
        /// Exit after this many seconds without connections (0 = never).
        #[arg(long, env = "KEPLER_IDLE_TIMEOUT_SECS", default_value_t = 300)]
        idle_timeout_secs: u64,
        /// Do not persist anything (no SQLite).
        #[arg(long)]
        no_storage: bool,
    },
    /// Send one event (JSON envelope) and print the decision. Falls back to passthrough.
    Emit {
        /// Read the event from a file ("-" or no source = stdin).
        #[arg(long, conflicts_with = "json")]
        file: Option<PathBuf>,
        /// Event JSON given inline.
        #[arg(long)]
        json: Option<String>,
        /// Do not start a daemon when none is running.
        #[arg(long)]
        no_autostart: bool,
        /// Evaluate locally without any daemon or persistence.
        #[arg(long)]
        in_process: bool,
    },
    /// Recent event metadata, memory counters and token metrics (read-only).
    Inspect {
        /// How many recent events/metrics to show.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Harness hook adapters (Claude Code): stdin JSON in, hook output JSON out, never fatal.
    Hook {
        #[command(subcommand)]
        cmd: hook_cmd::HookCmd,
    },
    /// Model Context Protocol server.
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Inspect, search, export, purge or disable local memory.
    Memory {
        #[command(subcommand)]
        cmd: memory_cmd::MemoryCmd,
    },
    /// Deterministic audit of code changes (no model calls; reads git diff only).
    Sentinel {
        #[command(subcommand)]
        cmd: sentinel_cmd::SentinelCmd,
    },
    /// On-demand UI inspection with a Playwright worker (off until a worker is configured).
    Vision {
        #[command(subcommand)]
        cmd: vision_cmd::VisionCmd,
    },
    /// Run a DAG of command tasks in isolated git worktrees, with approval checkpoints.
    Orbit {
        #[command(subcommand)]
        cmd: orbit_cmd::OrbitCmd,
    },
    /// Local read-only dashboard on 127.0.0.1 (random port, session token in the URL).
    Ui(ui_cmd::UiArgs),
    /// Print version information.
    Version {
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { json } => {
            version(json);
            ExitCode::SUCCESS
        }
        Command::Emit {
            file,
            json,
            no_autostart,
            in_process,
        } => {
            let raw = match read_event_source(file, json) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("kepler: {e}");
                    return ExitCode::from(2);
                }
            };
            emit(&raw, no_autostart, in_process)
        }
        Command::Hook { cmd } => hook_cmd::run(cmd),
        Command::Memory { cmd } => memory_cmd::run(cmd),
        Command::Sentinel { cmd } => sentinel_cmd::run(cmd),
        Command::Vision { cmd } => vision_cmd::run(cmd),
        Command::Orbit { cmd } => orbit_cmd::run(cmd),
        Command::Ui(args) => ui_cmd::run(args),
        Command::Mcp { cmd: McpCmd::Serve } => mcp_cmd::serve(),
        Command::Inspect { limit } => match inspect::inspect_value(limit) {
            Ok(v) => {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("kepler: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Doctor { json } => block_on(doctor_cmd(json)),
        Command::Daemon {
            idle_timeout_secs,
            no_storage,
        } => block_on(daemon_cmd(idle_timeout_secs, no_storage)),
    }
}

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    // Current-thread runtime: lowest idle footprint; the daemon is I/O bound.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

fn block_on(fut: impl std::future::Future<Output = ExitCode>) -> ExitCode {
    match runtime() {
        Ok(rt) => rt.block_on(fut),
        Err(e) => {
            eprintln!("kepler: cannot start runtime: {e}");
            ExitCode::FAILURE
        }
    }
}

fn version(json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "version": kepler_core::VERSION,
                "protocol": PROTOCOL_VERSION,
                "event_schema": ENVELOPE_VERSION,
            })
        );
    } else {
        println!(
            "kepler {} (ipc protocol {PROTOCOL_VERSION}, event schema {ENVELOPE_VERSION})",
            kepler_core::VERSION
        );
    }
}

fn read_event_source(file: Option<PathBuf>, json: Option<String>) -> Result<Vec<u8>, String> {
    let limit = (MAX_ENVELOPE_BYTES + 1) as u64;
    let bytes = if let Some(j) = json {
        j.into_bytes()
    } else {
        let mut buf = Vec::new();
        match file {
            Some(p) if p.as_os_str() != "-" => {
                let f = std::fs::File::open(&p)
                    .map_err(|e| format!("cannot open {}: {e}", p.display()))?;
                f.take(limit)
                    .read_to_end(&mut buf)
                    .map_err(|e| e.to_string())?;
            }
            _ => {
                std::io::stdin()
                    .lock()
                    .take(limit)
                    .read_to_end(&mut buf)
                    .map_err(|e| e.to_string())?;
            }
        }
        buf
    };
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(format!("event larger than {MAX_ENVELOPE_BYTES} bytes"));
    }
    Ok(bytes)
}

fn emit(raw: &[u8], no_autostart: bool, in_process: bool) -> ExitCode {
    let event: serde_json::Value = match serde_json::from_slice(raw) {
        Ok(v) => v,
        Err(e) => {
            let out = EmitOutcome::Rejected {
                code: "malformed".into(),
                message: e.to_string(),
            };
            println!("{}", out.to_json());
            return ExitCode::from(2);
        }
    };
    let outcome = if in_process {
        emit_in_process(event)
    } else {
        let spawn = spawn_daemon;
        let opts = EmitOptions {
            spawn_daemon: if no_autostart { None } else { Some(&spawn) },
            ..EmitOptions::default()
        };
        match runtime() {
            Ok(rt) => rt.block_on(emit_one_shot(&Endpoint::from_env(), event, &opts)),
            // Even a broken runtime must not block the caller's work.
            Err(_) => EmitOutcome::Passthrough {
                reason: "runtime_unavailable".into(),
            },
        }
    };
    println!("{}", outcome.to_json());
    match outcome {
        EmitOutcome::Rejected { .. } => ExitCode::from(2),
        _ => ExitCode::SUCCESS,
    }
}

/// Start `kepler daemon` detached, with no stdio, so the caller never waits on it.
fn spawn_daemon() -> std::io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        detach::make_std_handles_non_inheritable();
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(|_child| ())
}

async fn doctor_cmd(json: bool) -> ExitCode {
    let db = kepler_storage::dirs::db_path().map_err(|e| e.to_string());
    let report = doctor::run(&Endpoint::from_env(), db.as_deref().map_err(Clone::clone)).await;
    if json {
        println!("{}", report.to_json());
    } else {
        print!("{}", report.to_text());
    }
    ExitCode::SUCCESS
}

async fn daemon_cmd(idle_timeout_secs: u64, no_storage: bool) -> ExitCode {
    let endpoint = Endpoint::from_env();
    let mut cfg = DaemonConfig::new(endpoint.clone());
    cfg.pipeline = kepler_core::PipelineConfig::from_env();
    for w in &cfg.pipeline.warnings {
        eprintln!("kepler: warning: {w}");
    }
    cfg.idle_shutdown = (idle_timeout_secs > 0).then(|| Duration::from_secs(idle_timeout_secs));
    if !no_storage {
        match kepler_storage::dirs::db_path() {
            Ok(p) => cfg.db_path = Some(p),
            Err(e) => eprintln!("kepler: persistence disabled: {e}"),
        }
    }
    let daemon = match Daemon::start(cfg) {
        Ok(d) => d,
        // Two clients raced to autostart: the other one won. Not an error.
        Err(DaemonError::AlreadyRunning) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kepler: {e}");
            return ExitCode::FAILURE;
        }
    };
    for w in &daemon.warnings {
        eprintln!("kepler: warning: {w}");
    }
    println!("kepler daemon listening on {}", endpoint.describe());
    let exit = daemon
        .serve(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    eprintln!("kepler: daemon stopped ({exit:?})");
    ExitCode::SUCCESS
}
