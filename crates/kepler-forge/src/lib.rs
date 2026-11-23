//! Forge (KPL-009/010): deterministic input classifier and derived spec.
//!
//! * Modes: passthrough, enrich, compile, decompose.
//! * The original text is never modified; Forge only returns separate, labeled notes.
//! * Inferences are never promoted to user instructions (see [`spec::Claim`]).
//! * `analyze` makes no model call, no network, no I/O: total and deterministic. The optional
//!   model-assisted intent check (ADR-0026) is only a contract here (`model`); the adapter runs it.

mod analyze;
mod directive;
pub mod model;
mod module;
pub mod spec;

pub use analyze::{analyze, Analysis, Confidence, Mode, RuleClass, TaskKind};
pub use directive::{memory_directive, MemoryDirective};
pub use module::ForgeModule;
