use serde::{Deserialize, Serialize};

/// Text produced by Kepler, never by the user. Always labeled `inferred`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Derived {
    pub source: String,
    pub text: String,
    pub provenance: String,
}

impl Derived {
    pub fn inferred(source: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            text: text.into(),
            provenance: "inferred".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Decision {
    /// Use the original, untouched.
    Passthrough { reason: String },
    /// Original plus separate derived context (never a replacement).
    Enrich { derived: Vec<Derived> },
    /// Needs explicit user/harness approval before running.
    RequireApproval { reason: String },
    /// Must not run.
    Deny { reason: String },
}

impl Decision {
    pub fn passthrough(reason: impl Into<String>) -> Self {
        Self::Passthrough {
            reason: reason.into(),
        }
    }
    pub fn is_passthrough(&self) -> bool {
        matches!(self, Self::Passthrough { .. })
    }
}

/// A decision plus any optional-module degradations that were swallowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    #[serde(flatten)]
    pub decision: Decision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degraded: Vec<String>,
}

impl Verdict {
    pub fn new(decision: Decision) -> Self {
        Self {
            decision,
            degraded: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_wire_shape() {
        let v = Verdict::new(Decision::passthrough("trivial_input"));
        assert_eq!(
            serde_json::to_string(&v).unwrap(),
            r#"{"action":"passthrough","reason":"trivial_input"}"#
        );
        let back: Verdict = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
        let d = Verdict {
            decision: Decision::Deny { reason: "x".into() },
            degraded: vec!["m: failed".into()],
        };
        let back: Verdict = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }
}
