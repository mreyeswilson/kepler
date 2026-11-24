//! Read-only analysis tools: Forge (input classifier) and Guardian (command/path verdicts).

use crate::tool::{obj, string, Hints, Tool};
use crate::Backend;
use serde_json::{json, Map, Value};

fn guardian_check(b: &dyn Backend, a: &Map<String, Value>) -> Result<String, String> {
    match (a.contains_key("command"), a.contains_key("path")) {
        (true, false) | (false, true) => b.op("kepler_guardian_check", a),
        _ => Err("pass exactly one of `command` or `path`".into()),
    }
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "kepler_forge_analyze",
            title: "Classify a request (Forge)",
            description: "Run Kepler's deterministic Forge classifier on a piece of text: returns the mode (passthrough, enrich, compile, decompose), why, and labeled derived notes (user-stated / extracted / inferred / unknown, never user instructions). Use it to judge whether a vague or long request needs clarifying questions or splitting before you start; if mode is decompose, consider kepler_orbit_suggest_plan. Local rules only: no model, no network, nothing stored.",
            schema: || obj(json!({"text": string("The request to analyze (max 16 KiB)", 16384)}), &["text"]),
            hints: Hints::READ,
            handler: None,
        },
        Tool {
            name: "kepler_guardian_check",
            title: "Ask the Guardian about a command or path",
            description: "Ask Kepler's Guardian what it would do with a shell command or a file path: allow, ask (needs the user's approval) or deny, with the reason. Use before running something risky (deleting, force-pushing, reading secrets). It only classifies; it never runs the command or touches the path. Give exactly one of `command` (a shell command line to classify) or `path`.",
            schema: || {
                obj(
                    json!({
                        "command": string("Shell command line to classify (not executed)", 4096),
                        "path": string("File path to classify as a read/write target (not touched)", 1024),
                    }),
                    &[],
                )
            },
            hints: Hints::READ,
            handler: Some(guardian_check),
        },
    ]
}
