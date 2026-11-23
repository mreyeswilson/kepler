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
mod preflight;
pub mod sensitive;

pub use classify::{classify_input, InputClass};
pub use decision::{Decision, Derived, Verdict};
pub use engine::{Enrichment, ModuleError, OptionalModule, PolicyEngine, MAX_MODULES};
pub use preflight::preflight_tool_call;
