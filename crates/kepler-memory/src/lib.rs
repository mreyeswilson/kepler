//! Memory (KPL-011/012): scoped FTS5 retrieval with relevance/freshness/trust ranking,
//! de-duplication, token-budgeted output, consolidation on settlement, provenance and user
//! controls. Persistence primitives (and the redaction that precedes every write) live in
//! `kepler-storage`; this crate is policy: what to retrieve, rank, keep and forget.
//!
//! Nothing here calls a model or the network.

mod admin;
mod directive;
mod module;
pub mod rank;
mod service;

pub use admin::{row_to_json, MemoryAdmin};
pub use directive::user_directives;
pub use module::MemoryModule;
pub use service::{Consolidation, Ctx, MemoryConfig, MemoryService, Retrieval};
