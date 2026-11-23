//! Deterministic policy engine.
//!
//! Principles (AGENTS.md / ADR-0004):
//! * The original user input is never modified; the engine only returns a
//!   [`Decision`] next to it. Enrichment, when it exists, is *derived* and
//!   labeled `inferred`.
//! * Passthrough is the default and the fallback for every optional-module
//!   failure (error, panic, over budget, oversized output).
//! * The security preflight is not optional and fails closed: if it cannot
//!   produce a verdict the answer is `deny`, never `passthrough`.
//! * No model or network calls. Classification is rule-based.

mod classify;
mod decision;
mod engine;
pub mod guardian;
mod preflight;
pub mod redact;
pub mod sensitive;
pub mod tokens;

pub use classify::{classify_input, InputClass};
pub use decision::{Advisory, AdvisoryLevel, Decision, Derived, Verdict};
pub use engine::{
    Enrichment, ModuleError, OptionalModule, OutputStage, PolicyEngine, Provenance, MAX_MODULES,
    MAX_STAGES, MAX_TOTAL_DERIVED_BYTES,
};
pub use guardian::{
    BudgetStage, Guardian, GuardianRules, DEFAULT_CONTEXT_BUDGET_TOKENS, MAX_RULES,
};
pub use preflight::preflight_tool_call;
pub use tokens::{estimate_tokens, Basis, ModelFamily, TokenCount};
