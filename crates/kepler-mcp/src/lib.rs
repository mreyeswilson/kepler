//! Minimal MCP server over stdio (KPL-014, extended by ADR-0030): JSON-RPC 2.0, one message per
//! line, tools only.
//!
//! Scope, stated plainly: MCP gives an agent *callable tools*. It does **not** intercept the
//! agent's prompts, tool calls or results; that needs a native hook (see ADR-0002). Tools are
//! declared in `tools/` (one file per family, one registry table), validated against their own
//! JSON Schema, annotated for client confirmation UIs, redacted and bounded.
//!
//! Implemented methods: `initialize`, `notifications/initialized`, `ping`, `tools/list`,
//! `tools/call`. Everything else is answered with JSON-RPC "method not found". No batching
//! (removed from the 2025-06-18 revision), no resources, prompts, sampling or logging.

mod schema;
mod tool;
mod tools;

pub use tool::{Hints, Tool};
pub use tools::{build_suggested_plan, plan_schema, registry};

use serde_json::{json, Map, Value};
use std::io::{BufRead, Write};

/// Protocol revisions this server can speak, newest first.
pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// Longest accepted input line (bytes); longer lines are rejected without being parsed.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// Longest tool output text (bytes); longer output is cut with a marker.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;
/// Longest error message returned to the model (bytes).
const MAX_ERROR_BYTES: usize = 800;
/// Marker appended to a clipped output.
pub const TRUNCATION_MARK: &str = "\n[output truncated by kepler]";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What the server needs from Kepler. Implemented by the CLI; mocked in tests.
pub trait Backend {
    /// Diagnostics (same data as `kepler doctor --json`).
    fn doctor(&self) -> Result<Value, String>;
    /// Recent event metadata, memory counters and labeled token metrics.
    fn inspect(&self, limit: u32) -> Result<Value, String>;
    /// Scoped, ranked, budgeted memory retrieval (read-only).
    fn memory_search(&self, q: &MemorySearch) -> Result<String, String>;
    /// Every other tool: `op` is the tool name, `args` already passed its schema. Returns the
    /// text for the model or a short error message. Backends that lack an operation keep the
    /// default.
    fn op(&self, op: &str, _args: &Map<String, Value>) -> Result<String, String> {
        Err(format!("`{op}` is not available in this build"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySearch {
    pub query: String,
    pub project: String,
    pub session: Option<String>,
    pub branch: Option<String>,
    pub budget_tokens: u32,
}

pub struct McpServer<B: Backend> {
    backend: B,
    server_version: String,
    tools: Vec<Tool>,
    read_only: bool,
}

fn error(id: Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

fn ok(id: Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn text_result(text: String, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": clip(text, MAX_TOOL_OUTPUT_BYTES)}], "isError": is_error})
}

fn clip(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str(TRUNCATION_MARK);
    }
    s
}

/// Every tool definition (what `tools/list` returns when not in read-only mode).
pub fn tool_definitions() -> Value {
    Value::Array(registry().iter().map(Tool::definition).collect())
}

impl<B: Backend> McpServer<B> {
    pub fn new(backend: B, server_version: impl Into<String>) -> Self {
        Self {
            backend,
            server_version: server_version.into(),
            tools: registry(),
            read_only: false,
        }
    }

    /// Fail-closed switch (`KEPLER_MCP_READONLY=1`): tools that are not read-only are neither
    /// listed nor callable.
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    fn visible(&self) -> impl Iterator<Item = &Tool> {
        self.tools
            .iter()
            .filter(|t| !self.read_only || t.hints.read_only)
    }

    /// Handle one input line. `None` = nothing to send (notifications).
    pub fn handle_line(&self, line: &str) -> Option<String> {
        if line.len() > MAX_LINE_BYTES {
            return Some(error(Value::Null, INVALID_REQUEST, "message too large"));
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Some(error(Value::Null, PARSE_ERROR, "parse error")),
        };
        let Some(obj) = msg.as_object() else {
            return Some(error(
                Value::Null,
                INVALID_REQUEST,
                "expected a JSON object (batching is not supported)",
            ));
        };
        let id = obj.get("id").cloned();
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return id.map(|id| error(id, INVALID_REQUEST, "jsonrpc must be \"2.0\""));
        }
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response or junk from the client: nothing to answer.
            return id
                .filter(|_| !obj.contains_key("result") && !obj.contains_key("error"))
                .map(|id| error(id, INVALID_REQUEST, "missing method"));
        };
        let params = obj.get("params");
        let Some(id) = id else {
            return None; // notification (initialized, cancelled, ...): never answered
        };
        if !matches!(id, Value::String(_) | Value::Number(_)) {
            return Some(error(Value::Null, INVALID_REQUEST, "invalid id"));
        }
        Some(match method {
            "initialize" => ok(id, self.initialize(params)),
            "ping" => ok(id, json!({})),
            "tools/list" => ok(
                id,
                json!({"tools": self.visible().map(Tool::definition).collect::<Vec<_>>()}),
            ),
            "tools/call" => match self.call(params) {
                Ok(v) => ok(id, v),
                Err(m) => error(id, INVALID_PARAMS, &m),
            },
            _ => error(id, METHOD_NOT_FOUND, "method not found"),
        })
    }

    fn initialize(&self, params: Option<&Value>) -> Value {
        let requested = params
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str);
        let version = match requested {
            Some(r) if SUPPORTED_PROTOCOLS.contains(&r) => r,
            _ => SUPPORTED_PROTOCOLS[0],
        };
        let instructions = if self.read_only {
            "Kepler is in read-only mode (KEPLER_MCP_READONLY): diagnostics, inspection, analysis and memory/finding reads only. MCP does not let Kepler intercept your prompts, tool calls or results; automatic features need a native harness adapter."
        } else {
            "Kepler tools. Read-only: diagnostics, inspection, Forge/Guardian/Sentinel analysis, memory search, Orbit planning and status. With effects (ask the user first): kepler_orbit_run/resume/cancel/cleanup/approve (Orbit runs sub-agents and commands in separate git worktrees), kepler_sentinel_check, kepler_vision_check, kepler_memory_add. For a large task that splits into independent parts, use kepler_orbit_suggest_plan, show the plan to the user, and only start it after they agree; do not start agents just because they exist. MCP does not let Kepler intercept your prompts, tool calls or results; automatic features need a native harness adapter."
        };
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "kepler", "version": self.server_version},
            "instructions": instructions,
        })
    }

    fn call(&self, params: Option<&Value>) -> Result<Value, String> {
        let p = params
            .and_then(Value::as_object)
            .ok_or("params must be an object")?;
        let name = p
            .get("name")
            .and_then(Value::as_str)
            .ok_or("missing tool name")?;
        let empty = Map::new();
        let args = match p.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(m)) => m,
            Some(_) => return Err("arguments must be an object".into()),
        };
        let tool = self
            .tools
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| format!("unknown tool `{name}`"))?;
        if self.read_only && !tool.hints.read_only {
            return Ok(text_result(
                format!("kepler error: `{name}` is disabled (KEPLER_MCP_READONLY is set)"),
                true,
            ));
        }
        // Tool failures are reported as results with isError (so the model can see them);
        // only unknown tools and malformed arguments are protocol errors.
        schema::validate(&(tool.schema)(), &Value::Object(args.clone()), "arguments")?;
        let out = tool.call(&self.backend, args);
        Ok(match out {
            Ok(text) => {
                let text = if tool.hints.redact {
                    // Bound the work the redactor does, then bound what leaves.
                    kepler_policy::redact::redact(&clip(text, MAX_TOOL_OUTPUT_BYTES * 2))
                } else {
                    text
                };
                text_result(text, false)
            }
            Err(e) => text_result(
                clip(
                    format!("kepler error: {}", kepler_policy::redact::redact(&e)),
                    MAX_ERROR_BYTES,
                ),
                true,
            ),
        })
    }

    /// Serve until EOF. Writes one response per line and flushes after each.
    pub fn serve<R: BufRead, W: Write>(&self, mut input: R, mut output: W) -> std::io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            // Bound the buffered line: read at most MAX_LINE_BYTES + 1 bytes per message.
            let n = read_bounded_line(&mut input, &mut line, MAX_LINE_BYTES + 1)?;
            if n == 0 {
                return Ok(());
            }
            let trimmed = line.trim_end_matches(['\n', '\r']);
            if trimmed.trim().is_empty() {
                continue;
            }
            if let Some(resp) = self.handle_line(trimmed) {
                output.write_all(resp.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
    }
}

/// Like `read_line` but never buffers more than `limit` bytes; an overlong line is consumed
/// to its end and returned truncated (so the caller rejects it) instead of growing memory.
fn read_bounded_line<R: BufRead>(
    r: &mut R,
    out: &mut String,
    limit: usize,
) -> std::io::Result<usize> {
    let mut bytes: Vec<u8> = Vec::new();
    let mut total = 0usize;
    loop {
        let (done, used) = {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            match buf.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    if bytes.len() < limit {
                        let take = (i + 1).min(limit - bytes.len());
                        bytes.extend_from_slice(&buf[..take]);
                    }
                    (true, i + 1)
                }
                None => {
                    if bytes.len() < limit {
                        let take = buf.len().min(limit - bytes.len());
                        bytes.extend_from_slice(&buf[..take]);
                    }
                    (false, buf.len())
                }
            }
        };
        r.consume(used);
        total += used;
        if done {
            break;
        }
    }
    *out = String::from_utf8_lossy(&bytes).into_owned();
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock;
    impl Backend for Mock {
        fn doctor(&self) -> Result<Value, String> {
            Ok(json!({"kepler": {"version": "test"}}))
        }
        fn inspect(&self, limit: u32) -> Result<Value, String> {
            Ok(json!({"limit": limit}))
        }
        fn memory_search(&self, q: &MemorySearch) -> Result<String, String> {
            if q.project == "boom" {
                return Err("database unavailable".into());
            }
            if q.query == "huge" {
                return Ok("x".repeat(100_000));
            }
            Ok(format!("{}|{}|{}", q.project, q.query, q.budget_tokens))
        }
        fn op(&self, op: &str, args: &Map<String, Value>) -> Result<String, String> {
            match (op, args.get("text").and_then(Value::as_str)) {
                ("kepler_forge_analyze", Some("leak")) => {
                    Ok(format!("found token ghp_{}", "a1B2".repeat(9)))
                }
                ("kepler_forge_analyze", Some("huge")) => Ok("y".repeat(200_000)),
                ("kepler_orbit_plan", _) => {
                    if args["plan"]["name"] == "bad" {
                        Err("dependency cycle: a -> b -> a".into())
                    } else {
                        Ok(json!({"ok": true}).to_string())
                    }
                }
                _ => Ok(json!({"op": op, "args": args}).to_string()),
            }
        }
    }

    fn server() -> McpServer<Mock> {
        McpServer::new(Mock, "9.9.9")
    }

    fn call(s: &McpServer<Mock>, line: &str) -> Value {
        serde_json::from_str(&s.handle_line(line).expect("response")).unwrap()
    }

    #[test]
    fn initialize_negotiates_known_versions_and_never_claims_interception() {
        let s = server();
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["serverInfo"]["name"], "kepler");
        assert_eq!(r["result"]["serverInfo"]["version"], "9.9.9");
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], false);
        assert!(r["result"]["capabilities"].get("resources").is_none());
        assert!(r["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("does not let Kepler intercept"));
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
        );
        assert_eq!(r["result"]["protocolVersion"], SUPPORTED_PROTOCOLS[0]);
        assert_eq!(r["id"], "a");
    }

    #[test]
    fn notifications_get_no_response_and_ping_works() {
        let s = server();
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none());
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","method":"anything"}"#)
            .is_none());
        assert_eq!(
            call(&s, r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#)["result"],
            json!({})
        );
    }

    #[test]
    fn tools_list_declares_every_tool_with_strict_schema_and_annotations() {
        let r = call(
            &server(),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        let expected = [
            "kepler_doctor",
            "kepler_inspect",
            "kepler_memory_search",
            "kepler_orbit_suggest_plan",
            "kepler_orbit_plan",
            "kepler_orbit_run",
            "kepler_orbit_list",
            "kepler_orbit_status",
            "kepler_orbit_logs",
            "kepler_orbit_approve",
            "kepler_orbit_deny",
            "kepler_orbit_resume",
            "kepler_orbit_cancel",
            "kepler_orbit_cleanup",
            "kepler_forge_analyze",
            "kepler_guardian_check",
            "kepler_sentinel_audit",
            "kepler_sentinel_list",
            "kepler_sentinel_check",
            "kepler_memory_list",
            "kepler_memory_export",
            "kepler_memory_add",
            "kepler_vision_status",
            "kepler_vision_check",
        ];
        assert_eq!(names, expected);
        let mut seen = std::collections::HashSet::new();
        for t in tools {
            assert!(seen.insert(t["name"].as_str().unwrap()), "duplicate name");
            let schema = &t["inputSchema"];
            assert_eq!(schema["type"], "object", "{t}");
            assert_eq!(schema["additionalProperties"], false, "{t}");
            assert!(t["description"].as_str().unwrap().len() > 40, "{t}");
            let a = &t["annotations"];
            for k in [
                "readOnlyHint",
                "destructiveHint",
                "idempotentHint",
                "openWorldHint",
            ] {
                assert!(a[k].is_boolean(), "{} lacks {k}", t["name"]);
            }
            // Every required property is declared, every declared property is typed.
            for r in schema["required"].as_array().unwrap() {
                assert!(schema["properties"][r.as_str().unwrap()].is_object(), "{t}");
            }
            for (k, p) in schema["properties"].as_object().unwrap() {
                assert!(
                    p["type"].is_string(),
                    "{} property {k} has no type",
                    t["name"]
                );
            }
            // A read-only tool is never also marked destructive.
            if a["readOnlyHint"] == true {
                assert_eq!(a["destructiveHint"], false, "{}", t["name"]);
            }
        }
        assert_eq!(
            tools[2]["inputSchema"]["required"],
            json!(["query", "project"])
        );
    }

    #[test]
    fn risk_levels_match_the_annotations() {
        let r = call(
            &server(),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        let by = |n: &str| {
            r["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == n)
                .unwrap()["annotations"]
                .clone()
        };
        for n in [
            "kepler_doctor",
            "kepler_inspect",
            "kepler_memory_search",
            "kepler_memory_list",
            "kepler_memory_export",
            "kepler_forge_analyze",
            "kepler_guardian_check",
            "kepler_sentinel_audit",
            "kepler_sentinel_list",
            "kepler_vision_status",
            "kepler_orbit_suggest_plan",
            "kepler_orbit_plan",
            "kepler_orbit_list",
            "kepler_orbit_status",
            "kepler_orbit_logs",
        ] {
            assert_eq!(by(n)["readOnlyHint"], true, "{n}");
        }
        for n in [
            "kepler_orbit_run",
            "kepler_orbit_approve",
            "kepler_orbit_resume",
            "kepler_orbit_cancel",
            "kepler_orbit_cleanup",
            "kepler_sentinel_check",
            "kepler_memory_add",
            "kepler_vision_check",
        ] {
            assert_eq!(by(n)["readOnlyHint"], false, "{n}");
        }
        for n in [
            "kepler_orbit_run",
            "kepler_orbit_approve",
            "kepler_orbit_cleanup",
            "kepler_orbit_cancel",
            "kepler_sentinel_check",
        ] {
            assert_eq!(by(n)["destructiveHint"], true, "{n}");
        }
        assert_eq!(by("kepler_orbit_deny")["destructiveHint"], false);
        assert_eq!(by("kepler_orbit_approve")["idempotentHint"], true);
    }

    #[test]
    fn read_only_mode_hides_and_refuses_everything_with_effects() {
        let s = server().with_read_only(true);
        let r = call(&s, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let tools = r["result"]["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        assert!(tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true));
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kepler_orbit_run","arguments":{"plan":{"name":"p","tasks":[{"id":"a","command":["git","status"]}]},"repo":"."}}}"#,
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("KEPLER_MCP_READONLY"));
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(r["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("read-only"));
    }

    fn tool_call(s: &McpServer<Mock>, name: &str, args: Value) -> Value {
        let line = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}});
        call(s, &line.to_string())
    }

    fn text(r: &Value) -> String {
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn every_tool_rejects_unknown_fields_and_wrong_types_as_invalid_params() {
        let s = server();
        let list = call(&s, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        for t in list["result"]["tools"].as_array().unwrap() {
            let r = tool_call(&s, t["name"].as_str().unwrap(), json!({"nope": 1}));
            assert_eq!(r["error"]["code"], INVALID_PARAMS, "{}", t["name"]);
            let m = r["error"]["message"].as_str().unwrap();
            assert!(m.contains("nope") || m.contains("is required"), "{m}");
        }
    }

    #[test]
    fn approve_and_cleanup_need_explicit_confirm() {
        let s = server();
        let r = tool_call(
            &s,
            "kepler_orbit_approve",
            json!({"run":"orb-1","task":"t"}),
        );
        assert_eq!(
            r["error"]["code"], INVALID_PARAMS,
            "confirm is required by the schema"
        );
        let r = tool_call(
            &s,
            "kepler_orbit_approve",
            json!({"run":"orb-1","task":"t","confirm":false}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(text(&r).contains("explicitly approved"));
        let r = tool_call(
            &s,
            "kepler_orbit_approve",
            json!({"run":"orb-1","task":"t","confirm":true}),
        );
        assert_eq!(r["result"]["isError"], false);
        assert!(text(&r).contains("kepler_orbit_approve"));
        let r = tool_call(
            &s,
            "kepler_orbit_cleanup",
            json!({"run":"orb-1","confirm":false}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(text(&r).contains("nothing removed"));
        let r = tool_call(
            &s,
            "kepler_orbit_cleanup",
            json!({"run":"orb-1","confirm":true}),
        );
        assert_eq!(r["result"]["isError"], false);
    }

    #[test]
    fn guardian_check_needs_exactly_one_of_command_or_path() {
        let s = server();
        for args in [json!({}), json!({"command":"ls","path":"a"})] {
            let r = tool_call(&s, "kepler_guardian_check", args);
            assert_eq!(r["result"]["isError"], true);
            assert!(text(&r).contains("exactly one"));
        }
        let r = tool_call(&s, "kepler_guardian_check", json!({"command":"ls"}));
        assert_eq!(r["result"]["isError"], false);
        let r = tool_call(
            &s,
            "kepler_sentinel_audit",
            json!({"repo":".","staged":true,"base":"main"}),
        );
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn suggest_plan_builds_validates_and_reports_invalid_plans() {
        let s = server();
        let r = tool_call(
            &s,
            "kepler_orbit_suggest_plan",
            json!({"objective":"Add login","subtasks":[
                {"id":"api","prompt":"do api"},{"id":"ui","prompt":"do ui"}],
                "integration_prompt":"merge them"}),
        );
        assert_eq!(r["result"]["isError"], false);
        let v: Value = serde_json::from_str(&text(&r)).unwrap();
        assert_eq!(v["plan"]["tasks"].as_array().unwrap().len(), 3);
        assert_eq!(v["validation"]["ok"], true);
        assert!(v["next"].as_str().unwrap().contains("agrees"));
        // The backend (Orbit) refuses the plan: the model sees a short error, not a plan.
        let r = tool_call(
            &s,
            "kepler_orbit_suggest_plan",
            json!({"objective":"bad","subtasks":[{"id":"a","prompt":"x"}]}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(text(&r).contains("cycle"));
    }

    #[test]
    fn outputs_are_redacted_clipped_and_errors_are_short() {
        let s = server();
        let r = tool_call(&s, "kepler_forge_analyze", json!({"text":"leak"}));
        let t = text(&r);
        assert!(t.contains("found token") && !t.contains("ghp_a1B2"), "{t}");
        let r = tool_call(&s, "kepler_forge_analyze", json!({"text":"huge"}));
        let t = text(&r);
        assert!(t.len() <= MAX_TOOL_OUTPUT_BYTES + TRUNCATION_MARK.len());
        assert!(t.ends_with(TRUNCATION_MARK));
        // An unsupported backend operation is a clean tool error.
        struct Bare;
        impl Backend for Bare {
            fn doctor(&self) -> Result<Value, String> {
                Err("x".into())
            }
            fn inspect(&self, _: u32) -> Result<Value, String> {
                Err("x".into())
            }
            fn memory_search(&self, _: &MemorySearch) -> Result<String, String> {
                Err("x".into())
            }
        }
        let b = McpServer::new(Bare, "0");
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kepler_vision_status"}}"#;
        let r: Value = serde_json::from_str(&b.handle_line(line).unwrap()).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(text(&r).contains("not available"));
    }

    #[test]
    fn tool_calls_return_bounded_text_content() {
        let s = server();
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"kepler_doctor"}}"#,
        );
        assert_eq!(r["result"]["isError"], false);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"version\": \"test\""));
        // Out-of-range numbers are rejected by the schema, not silently clamped.
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"kepler_inspect","arguments":{"limit":9999}}}"#,
        );
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"branch","project":"p1","budget_tokens":100}}}"#,
        );
        assert_eq!(r["result"]["content"][0]["text"], "p1|branch|100");
        let r = call(
            &s,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"huge","project":"p1"}}}"#,
        );
        let t = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            t.len() < MAX_TOOL_OUTPUT_BYTES + 64 && t.ends_with("[output truncated by kepler]")
        );
    }

    #[test]
    fn backend_failures_are_tool_errors_not_protocol_errors() {
        let r = call(
            &server(),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"x","project":"boom"}}}"#,
        );
        assert!(r.get("error").is_none());
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("database unavailable"));
    }

    #[test]
    fn protocol_errors() {
        let s = server();
        let e = |line: &str| call(&s, line)["error"]["code"].as_i64().unwrap();
        assert_eq!(e("not json"), PARSE_ERROR);
        assert_eq!(e("[1,2]"), INVALID_REQUEST);
        assert_eq!(
            e(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#),
            INVALID_REQUEST
        );
        assert_eq!(
            e(r#"{"jsonrpc":"2.0","id":1,"method":"resources/list"}"#),
            METHOD_NOT_FOUND
        );
        assert_eq!(
            e(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope"}}"#),
            INVALID_PARAMS
        );
        assert_eq!(
            e(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kepler_memory_search","arguments":{"query":"x"}}}"#
            ),
            INVALID_PARAMS
        );
        assert_eq!(
            e(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kepler_inspect","arguments":{"limit":"ten"}}}"#
            ),
            INVALID_PARAMS
        );
        assert_eq!(
            e(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"kepler_doctor","arguments":[1]}}"#
            ),
            INVALID_PARAMS
        );
        assert_eq!(
            e(r#"{"jsonrpc":"2.0","id":{"x":1},"method":"ping"}"#),
            INVALID_REQUEST
        );
        // responses from the client are ignored
        assert!(s
            .handle_line(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#)
            .is_none());
        let big = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"ping","x":"{}"}}"#,
            "a".repeat(MAX_LINE_BYTES)
        );
        assert_eq!(e(&big), INVALID_REQUEST);
    }

    #[test]
    fn serve_loop_answers_line_by_line_and_survives_garbage() {
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\ngarbage\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let mut out = Vec::new();
        server().serve(&input[..], &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[1]["error"]["code"], PARSE_ERROR);
        assert_eq!(lines[2]["id"], 2);
    }

    #[test]
    fn overlong_lines_do_not_grow_memory_or_break_the_loop() {
        let mut input = vec![b'a'; MAX_LINE_BYTES * 3];
        input.push(b'\n');
        input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
        let mut out = Vec::new();
        server().serve(&input[..], &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0]["error"].is_object());
        assert_eq!(lines[1]["id"], 1);
    }
}
