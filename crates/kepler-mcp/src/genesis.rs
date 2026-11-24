//! Genesis MCP tools (ADR-0027): definitions and argument parsing, kept in their own file so the
//! rest of the server stays untouched. The work itself is done by the [`crate::Backend`].
//!
//! * `kepler_genesis_design`: calls the user's own `claude -p` once and writes files under the
//!   Kepler data directory (not read-only, not destructive, not idempotent).
//! * `kepler_genesis_inspect`, `kepler_genesis_list`, `kepler_genesis_show`: read-only.

use serde_json::{json, Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenesisOp {
    Inspect {
        project: Option<String>,
    },
    Design {
        brief: String,
        project: Option<String>,
        candidates: u32,
    },
    List,
    Show {
        id: String,
        candidate: u32,
    },
}

pub const TOOL_NAMES: &[&str] = &[
    "kepler_genesis_inspect",
    "kepler_genesis_design",
    "kepler_genesis_list",
    "kepler_genesis_show",
];

pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

pub fn append_definitions(defs: &mut Value) {
    let Some(a) = defs.as_array_mut() else { return };
    a.extend([
        json!({
            "name": "kepler_genesis_inspect",
            "description": "Deterministic, local inspection of a frontend project: detected stack (React/Vue/Angular/Svelte/Next/Tailwind/MUI/shadcn...), existing design tokens (colors, fonts, spacing, radii), component library folders and conventions. No model call, no network, never reads .env or node_modules. Use it before designing or restyling UI to learn what the project already has.",
            "inputSchema": {"type": "object", "properties": {
                "project": {"type": "string", "description": "Project directory (default: the server's working directory). Prefer an absolute path."}
            }, "additionalProperties": false},
            "annotations": {"title": "Genesis: inspect project design context", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "kepler_genesis_design",
            "description": "Use ONLY when the user asks to design or redesign an interface (a screen, flow or component set). Asks the user's own Claude CLI once (can take 30-120 s) for 1-3 distinct UI specification candidates grounded in the project's stack, tokens and components, validates them, and writes spec.json plus static HTML wireframe previews (and PNGs when a browser is available) under the Kepler data directory. The brief (secrets redacted) and a summary of the stack/tokens are sent to Anthropic. Returns a bounded summary and file paths, not the HTML; call kepler_genesis_show for the full spec of the candidate the user picks. Not for ordinary code changes.",
            "inputSchema": {"type": "object", "properties": {
                "brief": {"type": "string", "description": "What to design, in the user's words (<= 2000 chars used)."},
                "project": {"type": "string", "description": "Project directory (default: the server's working directory). Prefer an absolute path."},
                "candidates": {"type": "integer", "minimum": 1, "maximum": 3, "description": "How many distinct candidates (default 2)."}
            }, "required": ["brief"], "additionalProperties": false},
            "annotations": {"title": "Genesis: generate UI design candidates", "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        }),
        json!({
            "name": "kepler_genesis_list",
            "description": "List saved Genesis designs (newest first): id, brief, candidate names, model. Read-only.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": {"title": "Genesis: list saved designs", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
        json!({
            "name": "kepler_genesis_show",
            "description": "Show a saved Genesis design: the full JSON specification of one candidate (layout, sections, components mapped to the project's library, states, accessibility, responsive notes, implementation plan by file) plus preview paths. Use it after the user chooses a candidate, then implement it with the project's own components and tokens. Read-only.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string", "description": "Design id from kepler_genesis_design or kepler_genesis_list."},
                "candidate": {"type": "integer", "minimum": 1, "maximum": 3, "description": "Candidate number (default 1)."}
            }, "required": ["id"], "additionalProperties": false},
            "annotations": {"title": "Genesis: show a design specification", "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }),
    ]);
}

fn opt_path(m: &Map<String, Value>) -> Result<Option<String>, String> {
    match m.get("project") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= 1024 && !s.contains('\0') => {
            Ok(Some(s.clone()))
        }
        _ => Err("`project` must be a non-empty string path of at most 1024 bytes".into()),
    }
}

fn opt_int(m: &Map<String, Value>, k: &str, default: u32, max: u32) -> Result<u32, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| format!("`{k}` must be an integer between 1 and {max}")),
    }
}

/// Parse the arguments of one Genesis tool call (`name` must satisfy [`is_tool`]).
pub fn parse(name: &str, args: &Map<String, Value>) -> Result<GenesisOp, String> {
    let allowed: &[&str] = match name {
        "kepler_genesis_inspect" => &["project"],
        "kepler_genesis_design" => &["brief", "project", "candidates"],
        "kepler_genesis_list" => &[],
        "kepler_genesis_show" => &["id", "candidate"],
        _ => return Err(format!("unknown tool `{name}`")),
    };
    if let Some(k) = args.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!(
            "unexpected argument `{}`",
            k.chars().take(32).collect::<String>()
        ));
    }
    Ok(match name {
        "kepler_genesis_inspect" => GenesisOp::Inspect {
            project: opt_path(args)?,
        },
        "kepler_genesis_design" => {
            let brief = match args.get("brief") {
                Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= 8192 => s.clone(),
                _ => return Err("`brief` is required (non-empty string, <= 8192 bytes)".into()),
            };
            GenesisOp::Design {
                brief,
                project: opt_path(args)?,
                candidates: opt_int(args, "candidates", 2, 3)?,
            }
        }
        "kepler_genesis_list" => GenesisOp::List,
        _ => {
            let id = match args.get("id") {
                Some(Value::String(s)) if !s.is_empty() && s.len() <= 64 => s.clone(),
                _ => return Err("`id` is required (string, <= 64 bytes)".into()),
            };
            GenesisOp::Show {
                id,
                candidate: opt_int(args, "candidate", 1, 3)?,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn definitions_carry_honest_annotations() {
        let mut d = json!([]);
        append_definitions(&mut d);
        let tools = d.as_array().unwrap();
        assert_eq!(tools.len(), 4);
        for t in tools {
            assert!(is_tool(t["name"].as_str().unwrap()));
            assert_eq!(t["annotations"]["destructiveHint"], false);
            assert!(t["description"].as_str().unwrap().len() > 40);
        }
        let design = tools
            .iter()
            .find(|t| t["name"] == "kepler_genesis_design")
            .unwrap();
        assert_eq!(design["annotations"]["readOnlyHint"], false);
        assert_eq!(design["annotations"]["idempotentHint"], false);
        assert_eq!(design["annotations"]["openWorldHint"], true);
        assert!(design["description"]
            .as_str()
            .unwrap()
            .contains("user asks to design or redesign"));
        for n in [
            "kepler_genesis_inspect",
            "kepler_genesis_list",
            "kepler_genesis_show",
        ] {
            let t = tools.iter().find(|t| t["name"] == n).unwrap();
            assert_eq!(t["annotations"]["readOnlyHint"], true, "{n}");
        }
    }

    #[test]
    fn arguments_are_validated_and_bounded() {
        assert_eq!(
            parse("kepler_genesis_design", &args(json!({"brief": "login"}))),
            Ok(GenesisOp::Design {
                brief: "login".into(),
                project: None,
                candidates: 2
            })
        );
        assert!(parse("kepler_genesis_design", &args(json!({"brief": "  "}))).is_err());
        assert!(parse("kepler_genesis_design", &args(json!({}))).is_err());
        assert!(parse(
            "kepler_genesis_design",
            &args(json!({"brief": "x", "candidates": 4}))
        )
        .is_err());
        assert!(parse(
            "kepler_genesis_design",
            &args(json!({"brief": "x", "candidates": 0}))
        )
        .is_err());
        assert!(parse(
            "kepler_genesis_design",
            &args(json!({"brief": "x", "rm": "-rf"}))
        )
        .is_err());
        assert!(parse(
            "kepler_genesis_design",
            &args(json!({"brief": "x", "project": 5}))
        )
        .is_err());
        assert!(parse(
            "kepler_genesis_show",
            &args(json!({"id": "g1", "candidate": 2}))
        )
        .is_ok());
        assert!(parse("kepler_genesis_show", &args(json!({}))).is_err());
        assert_eq!(
            parse("kepler_genesis_list", &args(json!({}))),
            Ok(GenesisOp::List)
        );
        assert!(parse("kepler_genesis_list", &args(json!({"x": 1}))).is_err());
        assert!(parse("kepler_genesis_nope", &args(json!({}))).is_err());
    }
}
