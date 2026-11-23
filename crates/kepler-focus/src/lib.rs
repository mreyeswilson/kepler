//! Focus (KPL-026..031, KPL-033): deterministic execution supervision.
//!
//! * Scope lock: files the user named are remembered; unrelated edits raise a warning.
//! * Loop detector: repeated identical calls/failures without any change in between,
//!   oscillating edits.
//! * Execution budget: tool calls and elapsed time, progress aware; it only warns or
//!   recommends stopping, it never cancels (no harness interception is assumed).
//! * Minimal change guard: dependency manifests and very large change sets.
//! * Evidence/completion gate: whether a verification (test/build/check) was *observed*
//!   after the last change; unknown is reported as unknown.
//!
//! No model call, no I/O, no network. State is in memory, bounded and per session; the
//! user's prompt text is never stored (only a few file names mentioned in it). A Focus
//! failure can only lose advice: the engine contains panics and time budgets (ADR-0014).

mod config;
mod module;
mod state;

pub use config::FocusConfig;
pub use module::FocusModule;
