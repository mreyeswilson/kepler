//! Vision (KPL-016): on-demand UI inspection through a separate TypeScript/Playwright worker.
//!
//! * Lazy: nothing is spawned until a check is requested, and a request for a URL that is not
//!   on the allowlist never starts a worker.
//! * The worker speaks one JSON object per line over stdio (`protocol`), runs with a minimal
//!   environment, a wall-clock timeout per request (kill on expiry) and capped output.
//! * Only local URLs by default (`allowlist`).
//! * Results become `audit_findings` (`source = vision`); screenshots are opt-in files, never
//!   embedded in results or in the database.

mod allowlist;
mod findings;
mod manager;
mod protocol;

pub use allowlist::{check_url, parse_allowed_hosts, UrlError};
pub use findings::to_findings;
pub use manager::{CheckOptions, VisionError, WorkerCommand, WorkerManager};
pub use protocol::{CheckResult, Issue, Viewport, ViewportResult, PROTOCOL_VERSION};
