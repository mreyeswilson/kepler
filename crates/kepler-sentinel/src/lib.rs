//! Sentinel (KPL-015): deterministic audit of changes.
//!
//! * Path rules run in the daemon on `files.changed` (paths only: the daemon knows no
//!   repository root and never reads files for this).
//! * Diff rules run from the CLI (`kepler sentinel audit`) over `git diff` added lines.
//! * Findings are tied to a file, line and a short redacted evidence, separate **confirmed**
//!   patterns from **suspected** ones, and are stored in `audit_findings`.
//! * No model call, no network, no execution of project code. Every scan is bounded.

mod audit;
mod check;
mod diff;
mod module;
mod rules;

pub use audit::{audit_files, audit_repo, AuditError, AuditReport, AuditScope};
pub use check::{
    detect, parse_output as parse_check_output, run_check, CheckResult, CheckSpec, CheckStatus,
    DEFAULT_TIMEOUT as CHECK_TIMEOUT,
};
pub use diff::{parse_unified, FileDiff};
pub use module::SentinelModule;
pub use rules::{audit_diffs, audit_paths, Hit};
