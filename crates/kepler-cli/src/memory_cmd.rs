//! `kepler memory ...`: inspect, search, export, add, purge, disable/enable. Works on the
//! database file directly (no daemon needed); read-only commands never create it.

use clap::{Subcommand, ValueEnum};
use kepler_memory::{row_to_json, MemoryAdmin};
use kepler_storage::{MemoryFilter, PurgeFilter, Scope};
use std::process::ExitCode;

#[derive(Clone, Copy, ValueEnum)]
pub enum ScopeArg {
    Project,
    Session,
    Branch,
    User,
}

impl From<ScopeArg> for Scope {
    fn from(s: ScopeArg) -> Self {
        match s {
            ScopeArg::Project => Scope::Project,
            ScopeArg::Session => Scope::Session,
            ScopeArg::Branch => Scope::Branch,
            ScopeArg::User => Scope::User,
        }
    }
}

#[derive(Subcommand)]
pub enum MemoryCmd {
    /// List stored memories (newest first) with provenance.
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        /// Include memories replaced by newer ones.
        #[arg(long)]
        include_superseded: bool,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Counts, and whether memory is enabled.
    Stats {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Run a retrieval exactly like the daemon would (scoped, ranked, budgeted). Read-only.
    Search {
        query: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// Estimated-token budget.
        #[arg(long, default_value_t = 600)]
        budget: u32,
    },
    /// Export memories as JSON Lines to stdout (content re-redacted).
    Export {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        #[arg(long)]
        include_superseded: bool,
    },
    /// Store a memory by hand (source `user`, trust 90).
    Add {
        text: String,
        #[arg(long)]
        project: String,
        #[arg(long, value_enum, default_value = "project")]
        scope: ScopeArg,
        /// Session id (scope session) or branch name (scope branch).
        #[arg(long)]
        key: Option<String>,
    },
    /// Delete memories. Needs at least one filter or --all, and --yes.
    Purge {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, value_enum)]
        scope: Option<ScopeArg>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        older_than_days: Option<u32>,
        #[arg(long)]
        superseded: bool,
        /// Purge everything (together with --yes).
        #[arg(long)]
        all: bool,
        /// Confirm the deletion; without it nothing is deleted.
        #[arg(long)]
        yes: bool,
    },
    /// Stop retrieving and consolidating memory (globally, or for one project).
    Disable {
        #[arg(long)]
        project: Option<String>,
    },
    /// Re-enable memory.
    Enable {
        #[arg(long)]
        project: Option<String>,
    },
}

pub fn run(cmd: MemoryCmd) -> ExitCode {
    let path = match kepler_storage::dirs::db_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("kepler: {e}");
            return ExitCode::FAILURE;
        }
    };
    let admin = MemoryAdmin::new(path);
    match exec(&admin, cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kepler: {e}");
            ExitCode::FAILURE
        }
    }
}

fn exec(admin: &MemoryAdmin, cmd: MemoryCmd) -> Result<(), String> {
    let e = |e: kepler_storage::StorageError| e.to_string();
    match cmd {
        MemoryCmd::List {
            project,
            scope,
            include_superseded,
            limit,
            json,
        } => {
            let rows = admin
                .list(&MemoryFilter {
                    project_id: project,
                    scope: scope.map(Into::into),
                    include_superseded,
                    limit,
                })
                .map_err(e)?;
            if json {
                let v: Vec<_> = rows.iter().map(row_to_json).collect();
                println!("{}", serde_json::Value::Array(v));
            } else if rows.is_empty() {
                println!("no memories");
            } else {
                for r in &rows {
                    let sup = r
                        .superseded_by
                        .map_or(String::new(), |n| format!(" superseded_by={n}"));
                    let key = if r.scope_key.is_empty() {
                        String::new()
                    } else {
                        format!(":{}", r.scope_key)
                    };
                    println!(
                        "#{} [{}{}] project={} trust={} source={} used={}x updated={}{}\n    {}",
                        r.id,
                        r.scope.as_str(),
                        key,
                        r.project_id,
                        r.trust,
                        r.source,
                        r.use_count,
                        r.updated_at,
                        sup,
                        r.content.replace('\n', " ")
                    );
                    for s in &r.sources {
                        println!(
                            "    provenance: {} {} {}",
                            s.kind, s.reference, s.session_id
                        );
                    }
                }
            }
        }
        MemoryCmd::Stats { project, json } => {
            let s = admin.stats(project.as_deref()).map_err(e)?;
            let enabled = admin.is_enabled(project.as_deref()).map_err(e)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "enabled": enabled, "total": s.total, "live": s.live,
                        "superseded": s.superseded, "user_sourced": s.user_sourced,
                        "provenance_rows": s.sources,
                    })
                );
            } else {
                println!(
                    "memory {} | total {} (live {}, superseded {}, user-sourced {}) | provenance rows {}",
                    if enabled { "enabled" } else { "DISABLED" },
                    s.total,
                    s.live,
                    s.superseded,
                    s.user_sourced,
                    s.sources
                );
            }
        }
        MemoryCmd::Search {
            query,
            project,
            session,
            branch,
            budget,
        } => {
            let (block, sel) = admin
                .search(
                    &project,
                    session.as_deref(),
                    branch.as_deref(),
                    &query,
                    budget,
                )
                .map_err(e)?;
            match block {
                Some(b) => println!("{b}"),
                None => println!("no relevant memories"),
            }
            eprintln!(
                "({} item(s), ~{} tokens ESTIMATED, considered {}, duplicates {}, over budget {}, below threshold {})",
                sel.items.len(),
                sel.tokens,
                sel.stats.considered,
                sel.stats.duplicates,
                sel.stats.over_budget,
                sel.stats.below_threshold
            );
        }
        MemoryCmd::Export {
            project,
            scope,
            include_superseded,
        } => {
            let out = admin
                .export_jsonl(&MemoryFilter {
                    project_id: project,
                    scope: scope.map(Into::into),
                    include_superseded,
                    limit: 1000,
                })
                .map_err(e)?;
            print!("{out}");
        }
        MemoryCmd::Add {
            text,
            project,
            scope,
            key,
        } => {
            let o = admin
                .add(&project, scope.into(), key.as_deref().unwrap_or(""), &text)
                .map_err(e)?;
            println!("stored: {o:?}");
        }
        MemoryCmd::Purge {
            project,
            scope,
            session,
            older_than_days,
            superseded,
            all,
            yes,
        } => {
            let filter = PurgeFilter {
                project_id: project,
                scope: scope.map(Into::into),
                session_id: session,
                older_than_days,
                superseded_only: superseded,
                all,
            };
            if !yes {
                return Err(
                    "purge deletes data permanently; re-run with --yes to confirm (nothing was deleted)"
                        .into(),
                );
            }
            let n = admin.purge(filter).map_err(e)?;
            println!("purged {n} memor{}", if n == 1 { "y" } else { "ies" });
        }
        MemoryCmd::Disable { project } => {
            admin.set_enabled(project.as_deref(), false).map_err(e)?;
            println!(
                "memory disabled for {} (the daemon notices within a few seconds)",
                project.as_deref().unwrap_or("all projects")
            );
        }
        MemoryCmd::Enable { project } => {
            admin.set_enabled(project.as_deref(), true).map_err(e)?;
            println!(
                "memory enabled for {}",
                project.as_deref().unwrap_or("all projects")
            );
        }
    }
    Ok(())
}
