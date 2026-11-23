use crate::redact::redact;
use kepler_events::Envelope;
use kepler_policy::{Decision, Verdict};

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
            Decision::Redact { reason, .. } => ("redact", Some(reason.as_str())),
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

/// Legacy simple memory record (project scope). Content is redacted before it reaches
/// the database; new code should use [`crate::NewMemory`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    pub project_id: String,
    pub content: String,
    /// `user` (confirmed) or `derived` (inferred). Never promote one to the other silently.
    pub source: String,
    /// Where the fact came from, if a file. Sensitive paths are refused.
    pub source_path: Option<String>,
}
