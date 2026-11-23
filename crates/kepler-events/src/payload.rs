use crate::envelope::EventKind;
use crate::error::EventError;
use crate::limits::*;
use crate::validate::check_text;
use serde::{Deserialize, Serialize};

/// Bounded, typed payload. One variant per [`EventKind`]; there is no
/// free-form "instructions" variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Payload {
    InputReceived(InputReceived),
    PromptPrepared(PromptPrepared),
    ContextRequested(ContextRequested),
    ContextPrepared(ContextPrepared),
    ToolCallRequested(ToolCallRequested),
    ToolCallCompleted(ToolCallCompleted),
    FilesChanged(FilesChanged),
    AgentStarted(AgentStarted),
    AgentSettled(AgentSettled),
    MemoryCandidate(MemoryCandidate),
    SessionClosed(SessionClosed),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputReceived {
    /// The user's verbatim text. Never rewritten by Kepler.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPrepared {
    pub bytes: u32,
    /// True when Kepler-derived (inferred) context was added next to the original.
    #[serde(default)]
    pub derived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequested {
    pub budget_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextPrepared {
    pub items: u32,
    pub tokens: u32,
    /// `true` when `tokens` is an estimate rather than a harness measurement.
    pub estimated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallRequested {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Harness-declared hint. Treated as a hint only; core classifies on its own.
    #[serde(default)]
    pub destructive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCallCompleted {
    pub tool: String,
    pub ok: bool,
    pub output_bytes: u32,
    pub duration_ms: u32,
    /// Optional excerpt for in-memory inspection. Never persisted by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesChanged {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStarted {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettled {
    pub outcome: Outcome,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryCandidate {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionClosed {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn paths_ok(field: &'static str, paths: &[String]) -> Result<(), EventError> {
    if paths.len() > MAX_PATHS {
        return Err(EventError::FieldTooLarge {
            field,
            max: MAX_PATHS,
        });
    }
    paths
        .iter()
        .try_for_each(|p| check_text(field, p, MAX_PATH_BYTES))
}

fn opt(field: &'static str, v: &Option<String>, max: usize) -> Result<(), EventError> {
    v.as_deref().map_or(Ok(()), |s| check_text(field, s, max))
}

fn nonempty_tool(tool: &str) -> Result<(), EventError> {
    check_text("payload.tool", tool, MAX_TOOL_NAME_BYTES)?;
    if tool.is_empty() {
        return Err(EventError::field("payload.tool", "empty"));
    }
    Ok(())
}

impl Payload {
    /// Parse and validate the payload for a given kind (strict: unknown fields rejected).
    pub(crate) fn from_value(kind: EventKind, v: serde_json::Value) -> Result<Self, EventError> {
        fn de<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, EventError> {
            serde_json::from_value(v).map_err(|e| EventError::field("payload", e.to_string()))
        }
        let p = match kind {
            EventKind::InputReceived => Self::InputReceived(de(v)?),
            EventKind::PromptPrepared => Self::PromptPrepared(de(v)?),
            EventKind::ContextRequested => Self::ContextRequested(de(v)?),
            EventKind::ContextPrepared => Self::ContextPrepared(de(v)?),
            EventKind::ToolCallRequested => Self::ToolCallRequested(de(v)?),
            EventKind::ToolCallCompleted => Self::ToolCallCompleted(de(v)?),
            EventKind::FilesChanged => Self::FilesChanged(de(v)?),
            EventKind::AgentStarted => Self::AgentStarted(de(v)?),
            EventKind::AgentSettled => Self::AgentSettled(de(v)?),
            EventKind::MemoryCandidate => Self::MemoryCandidate(de(v)?),
            EventKind::SessionClosed => Self::SessionClosed(de(v)?),
        };
        p.validate()?;
        Ok(p)
    }

    pub fn kind(&self) -> EventKind {
        match self {
            Self::InputReceived(_) => EventKind::InputReceived,
            Self::PromptPrepared(_) => EventKind::PromptPrepared,
            Self::ContextRequested(_) => EventKind::ContextRequested,
            Self::ContextPrepared(_) => EventKind::ContextPrepared,
            Self::ToolCallRequested(_) => EventKind::ToolCallRequested,
            Self::ToolCallCompleted(_) => EventKind::ToolCallCompleted,
            Self::FilesChanged(_) => EventKind::FilesChanged,
            Self::AgentStarted(_) => EventKind::AgentStarted,
            Self::AgentSettled(_) => EventKind::AgentSettled,
            Self::MemoryCandidate(_) => EventKind::MemoryCandidate,
            Self::SessionClosed(_) => EventKind::SessionClosed,
        }
    }

    /// Size in bytes of the dominant free-text field, if any (for metadata-only persistence).
    pub fn text_bytes(&self) -> usize {
        match self {
            Self::InputReceived(p) => p.text.len(),
            Self::MemoryCandidate(p) => p.text.len(),
            Self::ToolCallRequested(p) => p.command.as_ref().map_or(0, String::len),
            Self::ToolCallCompleted(p) => p.output_bytes as usize,
            _ => 0,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), EventError> {
        match self {
            Self::InputReceived(p) => check_text("payload.text", &p.text, MAX_TEXT_BYTES),
            Self::ContextRequested(p) => opt("payload.query", &p.query, MAX_QUERY_BYTES),
            Self::ToolCallRequested(p) => {
                nonempty_tool(&p.tool)?;
                opt("payload.command", &p.command, MAX_COMMAND_BYTES)?;
                paths_ok("payload.paths", &p.paths)
            }
            Self::ToolCallCompleted(p) => {
                nonempty_tool(&p.tool)?;
                opt("payload.output_excerpt", &p.output_excerpt, MAX_TEXT_BYTES)
            }
            Self::FilesChanged(p) => paths_ok("payload.paths", &p.paths),
            Self::AgentStarted(p) => opt("payload.agent", &p.agent, MAX_SHORT_BYTES),
            Self::MemoryCandidate(p) => {
                check_text("payload.text", &p.text, MAX_MEMORY_TEXT_BYTES)?;
                opt("payload.category", &p.category, MAX_SHORT_BYTES)
            }
            Self::SessionClosed(p) => opt("payload.reason", &p.reason, MAX_SHORT_BYTES),
            Self::PromptPrepared(_) | Self::ContextPrepared(_) | Self::AgentSettled(_) => Ok(()),
        }
    }
}
