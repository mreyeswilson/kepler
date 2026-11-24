//! What a tool is: a name, a description that says WHEN to use it, a strict input schema, MCP
//! annotations and a handler. Each tool family lives in its own file under `tools/` and exports a
//! `tools()` function; `tools::registry()` is the single table the server reads. Adding a tool
//! (or a whole module, e.g. Genesis) is one new file plus one line in that table.

use crate::Backend;
use serde_json::{json, Map, Value};

/// MCP tool annotations (https://modelcontextprotocol.io/specification/2025-06-18/server/tools).
/// They are hints for the client's confirmation UI, not a security boundary: the boundary is
/// Kepler's own validation (schemas, `confirm` flags, Guardian) and `KEPLER_MCP_READONLY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hints {
    /// No side effects at all. Clients may run these without asking.
    pub read_only: bool,
    /// May overwrite, delete, kill or execute things (only meaningful when not read-only).
    pub destructive: bool,
    /// Calling again with the same arguments changes nothing more.
    pub idempotent: bool,
    /// Talks to things outside Kepler's local data (a browser, a model, project commands).
    pub open_world: bool,
    /// Pass the output through the secret redactor before it leaves the process.
    pub redact: bool,
}

impl Hints {
    /// Pure or read-only: safe to call without asking.
    pub const READ: Self = Self {
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
        redact: true,
    };
    /// Changes state or runs commands: clients should ask the user first.
    pub const WRITE: Self = Self {
        read_only: false,
        destructive: true,
        idempotent: false,
        open_world: false,
        redact: true,
    };
    pub const fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }
    pub const fn open_world(mut self) -> Self {
        self.open_world = true;
        self
    }
    pub const fn no_redact(mut self) -> Self {
        self.redact = false;
        self
    }
    pub const fn non_destructive(mut self) -> Self {
        self.destructive = false;
        self
    }
}

pub type Handler = fn(&dyn Backend, &Map<String, Value>) -> Result<String, String>;

pub struct Tool {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub hints: Hints,
    /// `None` forwards the validated arguments to `Backend::op(name, args)`.
    pub handler: Option<Handler>,
}

impl Tool {
    pub fn definition(&self) -> Value {
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": (self.schema)(),
            "annotations": {
                "title": self.title,
                "readOnlyHint": self.hints.read_only,
                "destructiveHint": self.hints.destructive,
                "idempotentHint": self.hints.idempotent,
                "openWorldHint": self.hints.open_world,
            },
        })
    }

    pub fn call(&self, backend: &dyn Backend, args: &Map<String, Value>) -> Result<String, String> {
        match self.handler {
            Some(h) => h(backend, args),
            None => backend.op(self.name, args),
        }
    }
}

// ---- schema builders shared by the tool files -------------------------------------------------

pub fn obj(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

pub fn string(description: &str, max: u32) -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": max, "description": description})
}

pub fn integer(description: &str, min: i64, max: i64) -> Value {
    json!({"type": "integer", "minimum": min, "maximum": max, "description": description})
}

pub fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

pub fn str_arg(args: &Map<String, Value>, k: &str) -> Option<String> {
    args.get(k).and_then(Value::as_str).map(str::to_string)
}

pub fn flag(args: &Map<String, Value>, k: &str) -> bool {
    args.get(k).and_then(Value::as_bool).unwrap_or(false)
}
