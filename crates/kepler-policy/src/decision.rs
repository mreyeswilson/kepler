use serde::{Deserialize, Serialize};

/// Text produced by Kepler, never by the user. Labeled `inferred` (rules) or `recalled` (memory).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Derived {
    pub source: String,
    pub text: String,
    pub provenance: String,
}

impl Derived {
    pub fn new(
        source: impl Into<String>,
        text: impl Into<String>,
        provenance: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            text: text.into(),
            provenance: provenance.into(),
        }
    }

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
    /// A tool result contains secrets: the harness should withhold or replace it.
    /// `kinds` are coarse categories (never the secret). Additive wire value (ADR-0011).
    Redact { reason: String, kinds: Vec<String> },
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

/// How strongly an [`Advisory`] asks for attention. Advisories never block anything by
/// themselves: blocking is reserved for the Guardian preflight (`deny` / `require_approval`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdvisoryLevel {
    Info,
    Warn,
    /// Kepler suggests the agent stop and report. A harness may ignore it; nothing is cancelled.
    RecommendStop,
}

/// Non-blocking supervision note (Focus, Sentinel). Additive wire field (ADR-0014); text is
/// Kepler-generated, bounded and never contains user prompt text or tool output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Advisory {
    pub source: String,
    pub level: AdvisoryLevel,
    /// Stable machine code such as `loop.repeated_failure`.
    pub code: String,
    pub message: String,
}

impl Advisory {
    pub fn new(
        source: impl Into<String>,
        level: AdvisoryLevel,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            level,
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A decision plus any optional-module degradations that were swallowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    #[serde(flatten)]
    pub decision: Decision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degraded: Vec<String>,
    /// Kepler's own *estimate* of the tokens in `derived` (never a measurement). Only on `enrich`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_tokens: Option<u32>,
    /// Non-blocking supervision notes (additive, omitted when empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisories: Vec<Advisory>,
}

impl Verdict {
    pub fn new(decision: Decision) -> Self {
        Self {
            decision,
            degraded: Vec::new(),
            estimated_tokens: None,
            advisories: Vec::new(),
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
            estimated_tokens: Some(12),
            advisories: vec![Advisory::new(
                "focus",
                AdvisoryLevel::Warn,
                "loop.repeated_call",
                "x",
            )],
        };
        let back: Verdict = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }
}
