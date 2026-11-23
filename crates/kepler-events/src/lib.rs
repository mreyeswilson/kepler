//! Versioned, bounded event envelope shared by every Kepler component.
//!
//! Events cross a trust boundary (adapter -> IPC -> core), so this crate is
//! deliberately strict: unknown versions, unknown fields, oversized input and
//! malformed identifiers are rejected with a stable [`EventError::code`].

mod envelope;
mod error;
mod payload;
mod validate;

pub use envelope::{Envelope, EventKind, ENVELOPE_VERSION, MAX_ENVELOPE_BYTES};
pub use error::EventError;
pub use payload::{
    AgentSettled, AgentStarted, ContextPrepared, ContextRequested, FilesChanged, InputReceived,
    MemoryCandidate, Outcome, Payload, PromptPrepared, SessionClosed, ToolCallCompleted,
    ToolCallRequested,
};
pub use validate::{is_valid_identifier, is_valid_rfc3339};

/// Limits for payload fields (bytes unless noted).
pub mod limits {
    pub const MAX_TEXT_BYTES: usize = 16 * 1024;
    pub const MAX_QUERY_BYTES: usize = 1024;
    pub const MAX_COMMAND_BYTES: usize = 4096;
    pub const MAX_TOOL_NAME_BYTES: usize = 128;
    pub const MAX_PATH_BYTES: usize = 1024;
    /// Maximum number of items in a path list (items, not bytes).
    pub const MAX_PATHS: usize = 64;
    pub const MAX_MEMORY_TEXT_BYTES: usize = 4096;
    pub const MAX_SHORT_BYTES: usize = 128;
    pub const MAX_IDENTIFIER_BYTES: usize = 128;
    pub const MAX_HARNESS_BYTES: usize = 64;
}
