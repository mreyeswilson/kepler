//! `kepler mcp serve`: MCP over stdio. Logs go to stderr only; stdout carries protocol messages.

use crate::inspect::{db_path, doctor_value, inspect_value};
use kepler_mcp::{Backend, McpServer, MemorySearch};
use kepler_memory::MemoryAdmin;
use serde_json::Value;
use std::io::{stdin, stdout, BufReader};
use std::process::ExitCode;

struct CliBackend;

impl Backend for CliBackend {
    fn doctor(&self) -> Result<Value, String> {
        doctor_value()
    }

    fn inspect(&self, limit: u32) -> Result<Value, String> {
        inspect_value(limit)
    }

    fn genesis(&self, op: &kepler_mcp::GenesisOp) -> Result<Value, String> {
        crate::genesis_cmd::mcp_call(op)
    }

    fn memory_search(&self, q: &MemorySearch) -> Result<String, String> {
        let admin = MemoryAdmin::new(db_path()?);
        if !admin
            .is_enabled(Some(&q.project))
            .map_err(|e| e.to_string())?
        {
            return Ok("memory is disabled (kepler memory enable)".into());
        }
        let (block, sel) = admin
            .search(
                &q.project,
                q.session.as_deref(),
                q.branch.as_deref(),
                &q.query,
                q.budget_tokens,
            )
            .map_err(|e| e.to_string())?;
        let note = format!(
            "\n({} item(s), ~{} tokens ESTIMATED)",
            sel.items.len(),
            sel.tokens
        );
        Ok(match block {
            Some(b) => b + &note,
            None => format!("no relevant memories{note}"),
        })
    }
}

pub fn serve() -> ExitCode {
    let server = McpServer::new(CliBackend, kepler_core::VERSION);
    match server.serve(BufReader::new(stdin().lock()), stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kepler: mcp: {e}");
            ExitCode::FAILURE
        }
    }
}
