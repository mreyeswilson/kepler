//! `kepler genesis ...` (ADR-0027): opt-in UI design specifications and static previews.
//! Nothing here runs unless the user types one of these commands (or the agent calls the
//! matching MCP tool). `design` is the only subcommand that calls a model: the user's own
//! `claude -p`, once, bounded by a deadline.

use clap::Subcommand;
use kepler_genesis::model::ClaudeCli;
use kepler_genesis::{
    design, inspect_project, list_json, show_candidate, show_outcome, DesignOptions,
};
use kepler_mcp::GenesisOp;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand)]
pub enum GenesisCmd {
    /// Deterministic inspection of the project's stack, design tokens and components (no model).
    Inspect {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Ask your own `claude -p` for 1-3 UI specification candidates and render safe HTML previews.
    /// Sends the brief (secrets redacted) and a summary of the stack/tokens to Anthropic.
    Design {
        /// What to design, e.g. "login screen with sign-up and password recovery".
        brief: String,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// Number of distinct candidates (1-3).
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=3))]
        candidates: u8,
        /// Do not try to capture PNG images of the previews.
        #[arg(long)]
        no_capture: bool,
        /// Ignore an identical saved design and ask the model again.
        #[arg(long)]
        fresh: bool,
        #[arg(long)]
        json: bool,
    },
    /// List saved designs (newest first).
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show a saved design: summary, or the full specification of one candidate.
    Show {
        id: String,
        /// Print the full specification of this candidate (1-3).
        #[arg(long)]
        candidate: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    /// Delete saved designs (all, or one id). Requires --yes.
    Purge {
        id: Option<String>,
        #[arg(long)]
        yes: bool,
    },
}

fn home() -> Result<PathBuf, String> {
    kepler_storage::dirs::data_dir().map_err(|e| e.to_string())
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn project_path(p: &Option<String>) -> PathBuf {
    PathBuf::from(p.as_deref().unwrap_or("."))
}

fn run_design(
    brief: &str,
    project: &Path,
    candidates: usize,
    capture: bool,
    fresh: bool,
) -> Result<Value, String> {
    let home = home()?;
    let model = ClaudeCli::from_env(Some(home.join("genesis-sandbox")));
    let opts = DesignOptions {
        brief: brief.to_string(),
        project: project.to_path_buf(),
        candidates,
        capture,
        fresh,
    };
    design(&home, &opts, &model).map_err(|e| e.to_string())
}

/// Entry point for the MCP backend.
pub fn mcp_call(op: &GenesisOp) -> Result<Value, String> {
    match op {
        GenesisOp::Inspect { project } => {
            let ctx = inspect_project(&project_path(project)).map_err(|e| e.to_string())?;
            serde_json::to_value(ctx).map_err(|e| e.to_string())
        }
        GenesisOp::Design {
            brief,
            project,
            candidates,
        } => run_design(
            brief,
            &project_path(project),
            *candidates as usize,
            true,
            false,
        ),
        GenesisOp::List => Ok(list_json(&home()?)),
        GenesisOp::Show { id, candidate } => {
            show_candidate(&home()?, id, *candidate as usize).map_err(|e| e.to_string())
        }
    }
}

fn print_outcome(v: &Value) {
    println!(
        "design {} ({}{}, {} ms)",
        v["id"].as_str().unwrap_or("?"),
        v["model"].as_str().unwrap_or("?"),
        if v["cached"] == true {
            ", from the saved design"
        } else {
            ""
        },
        v["latency_ms"]
    );
    println!("directory: {}", v["directory"].as_str().unwrap_or(""));
    for c in v["candidates"].as_array().into_iter().flatten() {
        println!(
            "\n[{}] {}\n    {}\n    screens: {}  components: {}",
            c["index"],
            c["name"].as_str().unwrap_or(""),
            c["rationale"].as_str().unwrap_or(""),
            c["screens"]
                .as_array()
                .map(|s| s
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
            c["components"]
        );
        if let Some(p) = c["preview_html"].as_str() {
            println!("    html: {p}");
        }
        match c["preview_png"].as_str() {
            Some(p) => println!("    png:  {p}"),
            None => println!("    png:  (not captured)"),
        }
        if c["unknown_tokens"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            println!(
                "    note: it refers to tokens not found in the project: {}",
                c["unknown_tokens"]
            );
        }
    }
    if let Some(m) = v["capture"]["method"].as_str() {
        println!("\ncapture: {m}");
    }
    for n in v["capture"]["notes"].as_array().into_iter().flatten() {
        println!("capture note: {}", n.as_str().unwrap_or(""));
    }
    for w in v["context_warnings"].as_array().into_iter().flatten() {
        println!("context: {}", w.as_str().unwrap_or(""));
    }
    println!(
        "\nFull specification: kepler genesis show {} --candidate N",
        v["id"].as_str().unwrap_or("ID")
    );
}

pub fn run(cmd: GenesisCmd) -> ExitCode {
    match exec(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kepler: genesis: {e}");
            ExitCode::from(2)
        }
    }
}

fn exec(cmd: GenesisCmd) -> Result<ExitCode, String> {
    match cmd {
        GenesisCmd::Inspect { project, json } => {
            let ctx = inspect_project(&project).map_err(|e| e.to_string())?;
            if json {
                println!(
                    "{}",
                    pretty(&serde_json::to_value(&ctx).map_err(|e| e.to_string())?)
                );
            } else {
                println!("{}", ctx.headline());
                println!(
                    "{}",
                    pretty(&serde_json::to_value(&ctx).map_err(|e| e.to_string())?)
                );
            }
        }
        GenesisCmd::Design {
            brief,
            project,
            candidates,
            no_capture,
            fresh,
            json,
        } => {
            let out = run_design(
                &brief,
                &project,
                usize::from(candidates),
                !no_capture,
                fresh,
            )?;
            if json {
                println!("{}", pretty(&out));
            } else {
                print_outcome(&out);
            }
        }
        GenesisCmd::List { json } => {
            let v = list_json(&home()?);
            if json {
                println!("{}", pretty(&v));
            } else {
                println!("{}", v["directory"].as_str().unwrap_or(""));
                let designs = v["designs"].as_array().cloned().unwrap_or_default();
                if designs.is_empty() {
                    println!("no saved designs");
                }
                for d in designs {
                    println!(
                        "{}  {} candidate(s), {} png  {}",
                        d["id"].as_str().unwrap_or("?"),
                        d["candidates"].as_array().map_or(0, Vec::len),
                        d["png_previews"],
                        d["brief"].as_str().unwrap_or("")
                    );
                }
            }
        }
        GenesisCmd::Show {
            id,
            candidate,
            json,
        } => {
            let home = home()?;
            let v = match candidate {
                Some(n) => show_candidate(&home, &id, n),
                None => show_outcome(&home, &id),
            }
            .map_err(|e| e.to_string())?;
            if json || candidate.is_some() {
                println!("{}", pretty(&v));
            } else {
                print_outcome(&v);
            }
        }
        GenesisCmd::Purge { id, yes } => {
            if !yes {
                return Err("purge deletes saved designs for good: pass --yes to confirm".into());
            }
            let store = kepler_genesis::store::Store::new(&home()?);
            match id {
                Some(id) => {
                    store.purge_one(&id).map_err(|e| e.to_string())?;
                    println!("{}", json!({"purged": [id]}));
                }
                None => println!("{}", json!({"purged_count": store.purge_all()})),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
