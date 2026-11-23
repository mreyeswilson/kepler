use crate::redact::redact;
use crate::StorageError;
use kepler_events::Envelope;
use kepler_policy::{sensitive::is_sensitive_path, Decision, Verdict};

pub const MAX_MEMORY_BYTES: usize = 4096;
const MAX_REASON_BYTES: usize = 128;

/// What is persisted about an event. Intentionally has no field that could hold
/// prompt text, a command line or tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub event_id: String,
    pub kind: String,
    pub ts: String,
    pub harness: String,
    pub project_id: String,
    pub session_id: String,
    pub correlation_id: String,
    pub payload_bytes: u32,
    pub decision: &'static str,
    pub reason: Option<String>,
}

impl EventRecord {
    pub fn from_event(e: &Envelope, v: &Verdict) -> Self {
        let (decision, reason) = match &v.decision {
            Decision::Passthrough { reason } => ("passthrough", Some(reason.as_str())),
            Decision::Enrich { .. } => ("enrich", None),
            Decision::RequireApproval { reason } => ("require_approval", Some(reason.as_str())),
            Decision::Deny { reason } => ("deny", Some(reason.as_str())),
        };
        Self {
            event_id: e.id.clone(),
            kind: e.kind.as_str().to_string(),
            ts: e.timestamp.clone(),
            harness: e.harness.clone(),
            project_id: e.project_id.clone(),
            session_id: e.session_id.clone(),
            correlation_id: e.correlation_id.clone(),
            payload_bytes: u32::try_from(e.payload.text_bytes()).unwrap_or(u32::MAX),
            decision,
            reason: reason.map(|r| redact(r).chars().take(MAX_REASON_BYTES).collect()),
        }
    }
}

/// A memory to store on purpose (future Memory module). Content is redacted
/// before it reaches the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    pub project_id: String,
    pub content: String,
    /// `user` (confirmed) or `derived` (inferred). Never promote one to the other silently.
    pub source: String,
    /// Where the fact came from, if a file. Sensitive paths are refused.
    pub source_path: Option<String>,
}

impl MemoryRecord {
    /// Validate and redact. Returns the sanitized content to store.
    pub(crate) fn sanitize(&self) -> Result<String, StorageError> {
        if let Some(p) = &self.source_path {
            if is_sensitive_path(p) {
                return Err(StorageError::Rejected(
                    "source path is on the sensitive-path denylist",
                ));
            }
        }
        if self.source != "user" && self.source != "derived" {
            return Err(StorageError::Rejected("source must be `user` or `derived`"));
        }
        if self.content.len() > MAX_MEMORY_BYTES {
            return Err(StorageError::Rejected("memory too large"));
        }
        let clean = redact(&self.content);
        if clean.trim().is_empty() {
            return Err(StorageError::Rejected("empty memory"));
        }
        Ok(clean)
    }
}
