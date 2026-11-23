//! Capability contract and negotiation (ADR-0002).
//!
//! An adapter advertises what its harness can really do. Negotiation never
//! guesses: every one of the eight capabilities ends up either `Granted` (with
//! the mechanism that backs it) or explicitly `Unsupported` (with a reason).
//!
//! Key rule: **MCP tools and CLI calls never provide transparent interception.**
//! `input_transform`, `context_filter`, `tool_preflight`, `tool_result_observe`
//! and `lifecycle_observe` are only granted for `native_hook` implementations.

use crate::limits::MAX_SHORT_BYTES;
use serde::{Deserialize, Serialize};

/// Maximum entries accepted in one advertisement (8 known + room for unknown ones).
pub const MAX_ADVERTISED_ENTRIES: usize = 32;
pub const MAX_CONSTRAINTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    InputTransform,
    ContextInject,
    ContextFilter,
    ToolPreflight,
    ToolResultObserve,
    LifecycleObserve,
    AgentSpawn,
    TokenUsageObserve,
}

impl Capability {
    pub const ALL: [Capability; 8] = [
        Capability::InputTransform,
        Capability::ContextInject,
        Capability::ContextFilter,
        Capability::ToolPreflight,
        Capability::ToolResultObserve,
        Capability::LifecycleObserve,
        Capability::AgentSpawn,
        Capability::TokenUsageObserve,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InputTransform => "input_transform",
            Self::ContextInject => "context_inject",
            Self::ContextFilter => "context_filter",
            Self::ToolPreflight => "tool_preflight",
            Self::ToolResultObserve => "tool_result_observe",
            Self::LifecycleObserve => "lifecycle_observe",
            Self::AgentSpawn => "agent_spawn",
            Self::TokenUsageObserve => "token_usage_observe",
        }
    }

    /// Capabilities that need the harness to call Kepler automatically at a
    /// hook point. Explicit tool/CLI calls cannot substitute for them.
    pub fn requires_native_hook(self) -> bool {
        matches!(
            self,
            Self::InputTransform
                | Self::ContextFilter
                | Self::ToolPreflight
                | Self::ToolResultObserve
                | Self::LifecycleObserve
        )
    }
}

/// How a capability is implemented by the harness/adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplKind {
    NativeHook,
    McpTool,
    Cli,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityEntry {
    /// Capability name as a string so unknown (future) names can be reported
    /// as unsupported instead of failing the whole handshake.
    pub capability: String,
    pub kind: ImplKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Advertisement {
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_version: Option<String>,
    pub capabilities: Vec<CapabilityEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Grant {
    Granted {
        kind: ImplKind,
        /// `true` when Kepler can only act when the model/user explicitly asks
        /// (MCP/CLI), i.e. no automatic interception.
        explicit_only: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        constraints: Vec<String>,
    },
    Unsupported {
        reason: String,
    },
}

impl Grant {
    pub fn is_granted(&self) -> bool {
        matches!(self, Self::Granted { .. })
    }
}

/// Result of negotiation: always exactly one verdict per known capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Negotiated {
    pub harness: String,
    pub results: Vec<(Capability, Grant)>,
    /// Advertised names Kepler does not know (ignored, reported).
    pub ignored: Vec<String>,
}

impl Negotiated {
    pub fn get(&self, c: Capability) -> &Grant {
        // `results` always holds all eight capabilities (constructed by `negotiate`).
        static MISSING: Grant = Grant::Unsupported {
            reason: String::new(),
        };
        self.results
            .iter()
            .find(|(k, _)| *k == c)
            .map_or(&MISSING, |(_, g)| g)
    }

    pub fn is_granted(&self, c: Capability) -> bool {
        self.get(c).is_granted()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    TooManyEntries,
    DuplicateCapability(String),
    InvalidField(&'static str),
}

impl std::fmt::Display for NegotiationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyEntries => write!(f, "too many capability entries"),
            Self::DuplicateCapability(c) => write!(f, "duplicate capability `{c}`"),
            Self::InvalidField(x) => write!(f, "invalid advertisement field `{x}`"),
        }
    }
}

impl std::error::Error for NegotiationError {}

/// Negotiate an adapter advertisement. Pure and deterministic.
pub fn negotiate(adv: &Advertisement) -> Result<Negotiated, NegotiationError> {
    if adv.capabilities.len() > MAX_ADVERTISED_ENTRIES {
        return Err(NegotiationError::TooManyEntries);
    }
    if !crate::is_valid_identifier(&adv.harness, crate::limits::MAX_HARNESS_BYTES) {
        return Err(NegotiationError::InvalidField("harness"));
    }
    if adv
        .harness_version
        .as_ref()
        .is_some_and(|v| v.len() > MAX_SHORT_BYTES)
    {
        return Err(NegotiationError::InvalidField("harness_version"));
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut ignored = Vec::new();
    for e in &adv.capabilities {
        if e.capability.len() > MAX_SHORT_BYTES
            || e.constraints.len() > MAX_CONSTRAINTS
            || e.constraints.iter().any(|c| c.len() > MAX_SHORT_BYTES)
            || e.version
                .as_ref()
                .is_some_and(|v| v.len() > MAX_SHORT_BYTES)
        {
            return Err(NegotiationError::InvalidField("capabilities"));
        }
        if seen.contains(&e.capability.as_str()) {
            return Err(NegotiationError::DuplicateCapability(
                e.capability.chars().take(48).collect(),
            ));
        }
        seen.push(&e.capability);
        if !Capability::ALL.iter().any(|c| c.as_str() == e.capability) {
            ignored.push(e.capability.clone());
        }
    }

    let results = Capability::ALL
        .iter()
        .map(|&cap| {
            let grant = match adv
                .capabilities
                .iter()
                .find(|e| e.capability == cap.as_str())
            {
                None => Grant::Unsupported {
                    reason: "not advertised by adapter".into(),
                },
                Some(e) => grant_for(cap, e),
            };
            (cap, grant)
        })
        .collect();
    Ok(Negotiated {
        harness: adv.harness.clone(),
        results,
        ignored,
    })
}

fn grant_for(cap: Capability, e: &CapabilityEntry) -> Grant {
    match e.kind {
        ImplKind::Unsupported => Grant::Unsupported {
            reason: "adapter declares it unsupported".into(),
        },
        ImplKind::McpTool | ImplKind::Cli if cap.requires_native_hook() => Grant::Unsupported {
            reason: format!(
                "{} requires a native hook; MCP/CLI cannot intercept automatically",
                cap.as_str()
            ),
        },
        kind => Grant::Granted {
            kind,
            explicit_only: kind != ImplKind::NativeHook,
            version: e.version.clone(),
            constraints: e.constraints.clone(),
        },
    }
}

/// Advertisement of a harness with nothing but a CLI (the universal fallback).
pub fn cli_only(harness: &str) -> Advertisement {
    Advertisement {
        harness: harness.to_string(),
        harness_version: None,
        capabilities: [Capability::ContextInject, Capability::AgentSpawn]
            .into_iter()
            .map(|c| CapabilityEntry {
                capability: c.as_str().into(),
                kind: ImplKind::Cli,
                version: None,
                constraints: vec![],
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(c: &str, kind: ImplKind) -> CapabilityEntry {
        CapabilityEntry {
            capability: c.into(),
            kind,
            version: None,
            constraints: vec![],
        }
    }

    fn adv(entries: Vec<CapabilityEntry>) -> Advertisement {
        Advertisement {
            harness: "pi".into(),
            harness_version: None,
            capabilities: entries,
        }
    }

    #[test]
    fn eight_capabilities_in_contract() {
        let names: Vec<_> = Capability::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(
            names,
            [
                "input_transform",
                "context_inject",
                "context_filter",
                "tool_preflight",
                "tool_result_observe",
                "lifecycle_observe",
                "agent_spawn",
                "token_usage_observe"
            ]
        );
        // wire names match serde names
        for c in Capability::ALL {
            assert_eq!(
                serde_json::to_string(&c).unwrap(),
                format!("\"{}\"", c.as_str())
            );
        }
    }

    #[test]
    fn empty_advertisement_is_all_unsupported_but_explicit() {
        let n = negotiate(&adv(vec![])).unwrap();
        assert_eq!(n.results.len(), 8);
        assert!(n.results.iter().all(|(_, g)| !g.is_granted()));
    }

    #[test]
    fn native_hook_is_granted_automatic() {
        let n = negotiate(&adv(vec![entry("input_transform", ImplKind::NativeHook)])).unwrap();
        match n.get(Capability::InputTransform) {
            Grant::Granted {
                explicit_only,
                kind,
                ..
            } => {
                assert!(!explicit_only);
                assert_eq!(*kind, ImplKind::NativeHook);
            }
            g => panic!("{g:?}"),
        }
    }

    #[test]
    fn mcp_never_grants_interception() {
        let entries: Vec<_> = Capability::ALL
            .iter()
            .map(|c| entry(c.as_str(), ImplKind::McpTool))
            .collect();
        let n = negotiate(&adv(entries)).unwrap();
        for c in Capability::ALL {
            assert_eq!(n.is_granted(c), !c.requires_native_hook(), "{}", c.as_str());
        }
        assert!(!n.is_granted(Capability::InputTransform));
        assert!(!n.is_granted(Capability::ToolPreflight));
        assert!(matches!(
            n.get(Capability::ContextInject),
            Grant::Granted {
                explicit_only: true,
                ..
            }
        ));
    }

    #[test]
    fn explicitly_unsupported_stays_unsupported() {
        let n = negotiate(&adv(vec![entry("agent_spawn", ImplKind::Unsupported)])).unwrap();
        assert!(matches!(
            n.get(Capability::AgentSpawn),
            Grant::Unsupported { .. }
        ));
    }

    #[test]
    fn unknown_capability_is_ignored_and_reported() {
        let n = negotiate(&adv(vec![entry("teleport", ImplKind::NativeHook)])).unwrap();
        assert_eq!(n.ignored, vec!["teleport".to_string()]);
        assert!(n.results.iter().all(|(_, g)| !g.is_granted()));
    }

    #[test]
    fn duplicates_and_oversize_are_rejected() {
        let dup = adv(vec![
            entry("tool_preflight", ImplKind::NativeHook),
            entry("tool_preflight", ImplKind::Cli),
        ]);
        assert!(matches!(
            negotiate(&dup),
            Err(NegotiationError::DuplicateCapability(_))
        ));
        let many = adv((0..33)
            .map(|i| entry(&format!("x{i}"), ImplKind::Cli))
            .collect());
        assert_eq!(negotiate(&many), Err(NegotiationError::TooManyEntries));
        let mut bad = adv(vec![]);
        bad.harness = "a b".into();
        assert!(negotiate(&bad).is_err());
    }

    #[test]
    fn cli_only_fallback() {
        let n = negotiate(&cli_only("any")).unwrap();
        assert!(n.is_granted(Capability::ContextInject));
        assert!(!n.is_granted(Capability::InputTransform));
    }

    #[test]
    fn advertisement_json_roundtrip_and_strictness() {
        let json = r#"{"harness":"pi","capabilities":[{"capability":"tool_preflight","kind":"native_hook","version":"1"}]}"#;
        let a: Advertisement = serde_json::from_str(json).unwrap();
        assert!(negotiate(&a).unwrap().is_granted(Capability::ToolPreflight));
        let bad = r#"{"harness":"pi","capabilities":[],"extra":1}"#;
        assert!(serde_json::from_str::<Advertisement>(bad).is_err());
    }
}
