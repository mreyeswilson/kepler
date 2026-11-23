//! Forge (KPL-009/010): deterministic input classifier and derived spec.
//!
//! * Modes: passthrough, enrich, compile, decompose.
//! * The original text is never modified; Forge only returns separate, labeled notes.
//! * Inferences are never promoted to user instructions (see [`spec::Claim`]).
//! * No model call, no network, no I/O. Every function is total and deterministic.

mod analyze;
mod directive;
mod module;
pub mod spec;

pub use analyze::{analyze, Analysis, Mode, TaskKind};
pub use directive::{memory_directive, MemoryDirective};
pub use module::ForgeModule;
