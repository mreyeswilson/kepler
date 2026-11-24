//! The tool table. One file per family; to add a module's tools (e.g. Genesis) add a file with
//! `pub fn tools() -> Vec<Tool>` and one `extend` line below.

mod analysis;
mod core;
mod memory;
mod orbit;
mod orbit_suggest;
mod sentinel;
mod vision;

pub use crate::tool::Tool;
pub use orbit::plan_schema;
pub use orbit_suggest::build as build_suggested_plan;

/// Every tool, in the order `tools/list` returns them.
pub fn registry() -> Vec<Tool> {
    let mut v = Vec::new();
    v.extend(core::tools());
    v.extend(orbit::tools());
    v.extend(analysis::tools());
    v.extend(sentinel::tools());
    v.extend(memory::tools());
    v.extend(vision::tools());
    v
}
