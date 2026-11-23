use std::fmt;

/// Why an event was rejected. `code()` values are part of the wire contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventError {
    /// Raw input exceeded the envelope size limit.
    Oversized { len: usize, max: usize },
    /// Not valid UTF-8 / JSON, or not a JSON object.
    Malformed(String),
    /// `version` is missing or not a supported number.
    UnsupportedVersion(String),
    /// `type` is not a known event kind.
    UnknownKind(String),
    /// A field is missing, has the wrong type, or an unknown field was present.
    InvalidField { field: &'static str, reason: String },
    /// A field exceeded its limit.
    FieldTooLarge { field: &'static str, max: usize },
}

impl EventError {
    /// Stable machine-readable code (shared with TypeScript fixtures).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Oversized { .. } => "oversized",
            Self::Malformed(_) => "malformed",
            Self::UnsupportedVersion(_) => "unsupported_version",
            Self::UnknownKind(_) => "unknown_kind",
            Self::InvalidField { .. } => "invalid_field",
            Self::FieldTooLarge { .. } => "field_too_large",
        }
    }

    pub(crate) fn field(field: &'static str, reason: impl Into<String>) -> Self {
        Self::InvalidField {
            field,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oversized { len, max } => write!(f, "event is {len} bytes, max is {max}"),
            Self::Malformed(m) => write!(f, "malformed event: {m}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported event version: {v}"),
            Self::UnknownKind(k) => write!(f, "unknown event type: {k}"),
            Self::InvalidField { field, reason } => write!(f, "invalid field `{field}`: {reason}"),
            Self::FieldTooLarge { field, max } => {
                write!(f, "field `{field}` exceeds {max} bytes/items")
            }
        }
    }
}

impl std::error::Error for EventError {}
