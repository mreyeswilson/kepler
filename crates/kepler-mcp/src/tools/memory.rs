//! Memory tools beyond search: list, export (read-only) and add (writes).

use crate::tool::{integer, obj, string, Hints, Tool};
use serde_json::json;

fn scope_prop() -> serde_json::Value {
    json!({"type": "string", "enum": ["project", "session", "branch"], "description": "Memory scope (default: all scopes when listing, project when adding)"})
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "kepler_memory_list",
            title: "List stored memories",
            description: "List Kepler's stored memories with provenance (newest first). Use to find project ids and see what Kepler remembers; use kepler_memory_search for relevance-ranked retrieval for a question.",
            schema: || {
                obj(
                    json!({
                        "project": string("Project id filter", 256),
                        "scope": scope_prop(),
                        "limit": integer("Max rows (default 30)", 1, 100),
                    }),
                    &[],
                )
            },
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_memory_export",
            title: "Export memories as JSON Lines",
            description: "Export stored memories as JSON Lines (content re-redacted, output truncated at the size limit). Use when the user wants a copy or an audit of what is stored; narrow with project/scope.",
            schema: || obj(json!({"project": string("Project id filter", 256), "scope": scope_prop()}), &[]),
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_memory_add",
            title: "Store a memory",
            description: "Store a durable project fact in Kepler's memory (recorded as agent-derived with lower trust than user-stated memories, redacted, bounded). Use only for stable facts worth recalling in later sessions (conventions, decisions the user stated) and tell the user what you stored. Never store secrets, credentials, or guesses presented as facts.",
            schema: || {
                obj(
                    json!({
                        "text": string("The fact, one or two sentences", 2000),
                        "project": string("Project id (see kepler_memory_list)", 256),
                        "scope": scope_prop(),
                        "key": string("Session id (scope session) or branch name (scope branch)", 256),
                    }),
                    &["text", "project"],
                )
            },
            hints: Hints::WRITE.non_destructive(),
            handler: None,
        },
    ]
}
