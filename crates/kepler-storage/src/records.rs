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
    /// Modules that produced output for this event (comma list). Names only.
    pub actors: Option<String>,
    /// Bounded `key=value;` codes (Forge mode, advisory codes, degraded modules). No user text.
    pub detail: Option<String>,
    /// Time the core spent evaluating the event.
    pub proc_ms: Option<u32>,
}

const MAX_DETAIL_BYTES: usize = 200;

impl EventRecord {
    /// Append a `key=value` code to `detail` (value restricted to `[A-Za-z0-9_.:,-]`, bounded).
    pub fn add_detail(&mut self, key: &str, value: &str) {
        let clean: String = value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | ',' | '-'))
            .take(64)
            .collect();
        let d = self.detail.get_or_insert_with(String::new);
        let add = format!("{key}={clean}");
        if d.len() + add.len() < MAX_DETAIL_BYTES {
            if !d.is_empty() {
                d.push(';');
            }
            d.push_str(&add);
        }
    }

    pub fn from_event(e: &Envelope, v: &Verdict) -> Self {
        let (decision, reason) = match &v.decision {
            Decision::Passthrough { reason } => ("passthrough", Some(reason.as_str())),
            Decision::Enrich { .. } => ("enrich", None),
            Decision::RequireApproval { reason } => ("require_approval", Some(reason.as_str())),
            Decision::Deny { reason } => ("deny", Some(reason.as_str())),
            Decision::Redact { reason, .. } => ("redact", Some(reason.as_str())),
        };
        let mut actors: Vec<String> = Vec::new();
        let mut push = |n: &str| {
            let n: String = n
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .take(24)
                .collect();
            if !n.is_empty() && !actors.contains(&n) {
                actors.push(n);
            }
        };
        if let Decision::Enrich { derived } = &v.decision {
            for d in derived {
                push(&d.source);
            }
        }
        for a in &v.advisories {
            push(&a.source);
        }
        if matches!(
            e.kind.as_str(),
            "tool.call.requested" | "tool.call.completed"
        ) {
            // The Guardian preflight / result scan runs on every tool event.
            push("guardian");
        }
        let mut rec = Self {
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
            actors: None,
            detail: None,
            proc_ms: None,
        };
        if !actors.is_empty() {
            rec.actors = Some(actors.join(","));
        }
        if !v.degraded.is_empty() {
            let names: Vec<&str> = v
                .degraded
                .iter()
                .filter_map(|d| d.split(':').next())
                .collect();
            rec.add_detail("degraded", &names.join(","));
        }
        for a in &v.advisories {
            rec.add_detail("adv", &a.code);
        }
        rec
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
