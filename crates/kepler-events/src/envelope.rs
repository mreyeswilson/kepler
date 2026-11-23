use crate::error::EventError;
use crate::limits::{MAX_HARNESS_BYTES, MAX_IDENTIFIER_BYTES};
use crate::payload::Payload;
use crate::validate::{check_identifier, is_valid_rfc3339};
use serde::Serialize;
use serde_json::{Map, Value};

pub const ENVELOPE_VERSION: u32 = 1;
/// Hard limit for one serialized envelope.
pub const MAX_ENVELOPE_BYTES: usize = 64 * 1024;

macro_rules! kinds {
    ($($variant:ident => $s:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum EventKind { $($variant),+ }
        impl EventKind {
            pub const ALL: &'static [EventKind] = &[$(EventKind::$variant),+];
            pub fn as_str(self) -> &'static str { match self { $(Self::$variant => $s),+ } }
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($s => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

kinds! {
    InputReceived => "input.received",
    PromptPrepared => "prompt.prepared",
    ContextRequested => "context.requested",
    ContextPrepared => "context.prepared",
    ToolCallRequested => "tool.call.requested",
    ToolCallCompleted => "tool.call.completed",
    FilesChanged => "files.changed",
    AgentStarted => "agent.started",
    AgentSettled => "agent.settled",
    MemoryCandidate => "memory.candidate",
    SessionClosed => "session.closed",
}

impl Serialize for EventKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// A validated event. Construct with [`Envelope::parse`] / [`Envelope::from_value`];
/// after mutating public fields call [`Envelope::validate`] again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Envelope {
    pub version: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: EventKind,
    pub timestamp: String,
    pub harness: String,
    pub project_id: String,
    pub session_id: String,
    pub correlation_id: String,
    pub payload: Payload,
}

const FIELDS: [&str; 9] = [
    "version",
    "id",
    "type",
    "timestamp",
    "harness",
    "project_id",
    "session_id",
    "correlation_id",
    "payload",
];

impl Envelope {
    /// Parse untrusted bytes. Enforces the size limit before touching the parser.
    pub fn parse(bytes: &[u8]) -> Result<Self, EventError> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            return Err(EventError::Oversized {
                len: bytes.len(),
                max: MAX_ENVELOPE_BYTES,
            });
        }
        let v: Value =
            serde_json::from_slice(bytes).map_err(|e| EventError::Malformed(e.to_string()))?;
        Self::from_value(v)
    }

    /// Validate an already-parsed JSON value (used by IPC, where the frame limit
    /// has already bounded the size).
    pub fn from_value(v: Value) -> Result<Self, EventError> {
        let Value::Object(mut obj) = v else {
            return Err(EventError::Malformed("event must be a JSON object".into()));
        };
        // Version first, so future versions are reported as such rather than as
        // an unknown-field error.
        match obj.get("version") {
            Some(Value::Number(n)) if n.as_u64() == Some(u64::from(ENVELOPE_VERSION)) => {}
            Some(other) => return Err(EventError::UnsupportedVersion(short(other))),
            None => return Err(EventError::UnsupportedVersion("missing".into())),
        }
        if let Some(k) = obj.keys().find(|k| !FIELDS.contains(&k.as_str())) {
            return Err(EventError::field(
                "envelope",
                format!("unknown field `{}`", clip(k)),
            ));
        }
        let kind_str = string(&mut obj, "type")?;
        let kind =
            EventKind::parse(&kind_str).ok_or_else(|| EventError::UnknownKind(clip(&kind_str)))?;
        let id = string(&mut obj, "id")?;
        let timestamp = string(&mut obj, "timestamp")?;
        let harness = string(&mut obj, "harness")?;
        let project_id = string(&mut obj, "project_id")?;
        let session_id = string(&mut obj, "session_id")?;
        let correlation_id = string(&mut obj, "correlation_id")?;
        let payload = obj
            .remove("payload")
            .ok_or_else(|| EventError::field("payload", "missing"))?;
        let env = Envelope {
            version: ENVELOPE_VERSION,
            id,
            kind,
            timestamp,
            harness,
            project_id,
            session_id,
            correlation_id,
            payload: Payload::from_value(kind, payload)?,
        };
        env.validate()?;
        Ok(env)
    }

    pub fn validate(&self) -> Result<(), EventError> {
        if self.version != ENVELOPE_VERSION {
            return Err(EventError::UnsupportedVersion(self.version.to_string()));
        }
        check_identifier("id", &self.id, MAX_IDENTIFIER_BYTES)?;
        check_identifier("project_id", &self.project_id, MAX_IDENTIFIER_BYTES)?;
        check_identifier("session_id", &self.session_id, MAX_IDENTIFIER_BYTES)?;
        check_identifier("correlation_id", &self.correlation_id, MAX_IDENTIFIER_BYTES)?;
        check_identifier("harness", &self.harness, MAX_HARNESS_BYTES)?;
        if !is_valid_rfc3339(&self.timestamp) {
            return Err(EventError::field("timestamp", "not RFC 3339"));
        }
        if self.payload.kind() != self.kind {
            return Err(EventError::field("payload", "does not match type"));
        }
        self.payload.validate()
    }

    pub fn to_json(&self) -> Result<String, EventError> {
        let s = serde_json::to_string(self).map_err(|e| EventError::Malformed(e.to_string()))?;
        if s.len() > MAX_ENVELOPE_BYTES {
            return Err(EventError::Oversized {
                len: s.len(),
                max: MAX_ENVELOPE_BYTES,
            });
        }
        Ok(s)
    }
}

fn string(obj: &mut Map<String, Value>, key: &'static str) -> Result<String, EventError> {
    match obj.remove(key) {
        Some(Value::String(s)) => Ok(s),
        Some(_) => Err(EventError::field(key, "must be a string")),
        None => Err(EventError::field(key, "missing")),
    }
}

fn clip(s: &str) -> String {
    s.chars().take(48).collect()
}

fn short(v: &Value) -> String {
    clip(&v.to_string())
}
