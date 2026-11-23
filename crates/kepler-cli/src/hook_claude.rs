//! Pure mapping between Claude Code hook payloads and Kepler v1 events (ADR-0025).
//!
//! No I/O here: the functions take parsed JSON and return JSON, so they are unit-tested with
//! fixtures. Anything that cannot be mapped yields `None` (= the hook stays silent = passthrough).

use kepler_core::EmitOutcome;
use kepler_events::limits::{MAX_COMMAND_BYTES, MAX_PATHS, MAX_PATH_BYTES, MAX_TEXT_BYTES};
use kepler_policy::Decision;
use serde_json::{json, Map, Value};

pub const HARNESS: &str = "claude-code";
/// Upper bound for the derived-context block handed back to Claude Code (chars).
/// Claude Code spills `additionalContext` above 10,000 characters to a file, so stay below.
pub const MAX_CONTEXT_BLOCK_CHARS: usize = 8000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    SessionStart,
    SessionEnd,
}

impl HookEvent {
    pub const ALL: [HookEvent; 6] = [
        Self::UserPromptSubmit,
        Self::PreToolUse,
        Self::PostToolUse,
        Self::Stop,
        Self::SessionStart,
        Self::SessionEnd,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::Stop => "Stop",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
        }
    }
}

/// Values that vary per call; injected so tests are deterministic.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub timestamp: String,
    pub nonce: String,
}

/// Make an arbitrary host string a valid Kepler identifier (`[A-Za-z0-9._:-]{1,128}`).
pub fn sanitize_identifier(raw: &str, fallback: &str) -> String {
    let clean: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if clean.is_empty() {
        fallback.to_string()
    } else {
        clean
    }
}

fn fnv1a(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Stable, pseudonymous project id derived from the working directory. The path itself is never
/// sent. Not cryptographic (two FNV-1a 64 passes): it only has to be stable and non-reversible
/// in practice for a local daemon.
pub fn project_id_from_cwd(cwd: &str) -> String {
    let mut n = cwd.replace('\\', "/");
    while n.len() > 1 && n.ends_with('/') {
        n.pop();
    }
    // Windows paths are case-insensitive; Unix ones are not. A drive letter marks Windows.
    let b = n.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        n = n.to_ascii_lowercase();
    }
    let a = fnv1a(0xcbf2_9ce4_8422_2325, n.as_bytes());
    let b2 = fnv1a(0x8422_2325_cbf2_9ce4, n.as_bytes());
    format!("cc-{a:016x}{b2:016x}")
}

fn clip(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn envelope(ev: HookEvent, kind: &str, payload: Value, input: &Value, ctx: &Ctx) -> Value {
    let id = format!("cc-{}", ctx.nonce);
    let session = sanitize_identifier(str_field(input, "session_id").unwrap_or(""), "cc-unknown");
    // Pair requested/completed of the same tool call, and tie a prompt to its events.
    let corr_src = match ev {
        HookEvent::PreToolUse | HookEvent::PostToolUse => str_field(input, "tool_use_id"),
        HookEvent::UserPromptSubmit => str_field(input, "prompt_id"),
        _ => None,
    };
    let correlation = corr_src
        .map(|c| format!("cc-{}", sanitize_identifier(c, "x")))
        .map(|c| clip(&c, 128).to_string())
        .unwrap_or_else(|| id.clone());
    let cwd = str_field(input, "cwd").unwrap_or("");
    json!({
        "version": 1,
        "id": id,
        "type": kind,
        "timestamp": ctx.timestamp,
        "harness": HARNESS,
        "project_id": project_id_from_cwd(cwd),
        "session_id": session,
        "correlation_id": correlation,
        "payload": payload,
    })
}

/// Paths that tools take in `tool_input`.
fn tool_paths(input: &Value) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut too_long = false;
    if let Some(ti) = input.get("tool_input").filter(|v| v.is_object()) {
        for k in ["file_path", "path", "notebook_path"] {
            if let Some(p) = ti.get(k).and_then(Value::as_str) {
                if p.is_empty() || p.contains('\0') {
                    continue;
                }
                if p.len() > MAX_PATH_BYTES {
                    too_long = true;
                } else if out.len() < MAX_PATHS {
                    out.push(p.to_string());
                }
            }
        }
    }
    (out, too_long)
}

/// Text of a `tool_response` for in-memory scanning. Bash responses expose `stdout`/`stderr`;
/// anything else is serialized compactly (bounded later).
fn response_text(resp: &Value) -> String {
    match resp {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Object(o) if o.contains_key("stdout") || o.contains_key("stderr") => {
            let mut parts = Vec::new();
            for k in ["stdout", "stderr"] {
                if let Some(s) = o.get(k).and_then(Value::as_str) {
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
            }
            parts.join("\n")
        }
        other => other.to_string(),
    }
}

fn response_ok(resp: &Value) -> bool {
    match resp {
        Value::Object(o) => {
            let flag = |k: &str| o.get(k).and_then(Value::as_bool) == Some(true);
            !(flag("interrupted") || flag("isError") || flag("is_error") || o.contains_key("error"))
        }
        _ => true,
    }
}

/// Map a hook payload to a v1 envelope (as JSON). `None` = nothing to report (passthrough).
pub fn build_envelope(ev: HookEvent, input: &Value, ctx: &Ctx) -> Option<Value> {
    if !input.is_object() {
        return None;
    }
    match ev {
        HookEvent::UserPromptSubmit => {
            let text = str_field(input, "prompt")?;
            // Verbatim or not at all: never truncated or rewritten.
            if text.is_empty() || text.len() > MAX_TEXT_BYTES || text.contains('\0') {
                return None;
            }
            Some(envelope(
                ev,
                "input.received",
                json!({ "text": text }),
                input,
                ctx,
            ))
        }
        HookEvent::PreToolUse => {
            let tool = clip(str_field(input, "tool_name")?, 128);
            if tool.is_empty() || tool.contains('\0') {
                return None;
            }
            let mut p = Map::new();
            p.insert("tool".into(), json!(tool));
            // Fail closed on what Kepler cannot see in full: a truncated command or path is
            // reported as a destructive hint, which Guardian turns into `require_approval`.
            let mut oversized = false;
            if let Some(cmd) = input
                .get("tool_input")
                .and_then(|t| t.get("command"))
                .and_then(Value::as_str)
            {
                if !cmd.contains('\0') {
                    if cmd.len() > MAX_COMMAND_BYTES {
                        oversized = true;
                    }
                    p.insert("command".into(), json!(clip(cmd, MAX_COMMAND_BYTES)));
                }
            }
            let (paths, too_long) = tool_paths(input);
            oversized |= too_long;
            if !paths.is_empty() {
                p.insert("paths".into(), json!(paths));
            }
            if oversized {
                p.insert("destructive".into(), json!(true));
            }
            Some(envelope(
                ev,
                "tool.call.requested",
                Value::Object(p),
                input,
                ctx,
            ))
        }
        HookEvent::PostToolUse => {
            let tool = clip(str_field(input, "tool_name")?, 128);
            if tool.is_empty() || tool.contains('\0') {
                return None;
            }
            let resp = input.get("tool_response").unwrap_or(&Value::Null);
            let text = response_text(resp);
            let mut p = Map::new();
            p.insert("tool".into(), json!(tool));
            p.insert("ok".into(), json!(response_ok(resp)));
            p.insert(
                "output_bytes".into(),
                json!(u32::try_from(text.len()).unwrap_or(u32::MAX)),
            );
            p.insert("duration_ms".into(), json!(0));
            if !text.is_empty() && !text.contains('\0') {
                p.insert("output_excerpt".into(), json!(clip(&text, MAX_TEXT_BYTES)));
            }
            Some(envelope(
                ev,
                "tool.call.completed",
                Value::Object(p),
                input,
                ctx,
            ))
        }
        HookEvent::Stop => Some(envelope(
            ev,
            "agent.settled",
            // Claude Code does not report an outcome or duration on Stop.
            json!({ "outcome": "ok", "duration_ms": 0 }),
            input,
            ctx,
        )),
        HookEvent::SessionStart => Some(envelope(
            ev,
            "agent.started",
            json!({ "agent": HARNESS }),
            input,
            ctx,
        )),
        HookEvent::SessionEnd => {
            let payload = match str_field(input, "reason").filter(|r| !r.contains('\0')) {
                Some(r) => json!({ "reason": clip(r, 128) }),
                None => json!({}),
            };
            Some(envelope(ev, "session.closed", payload, input, ctx))
        }
    }
}

/// Labeled, bounded block of Kepler-derived context. Never mixed into the user's prompt.
fn render_derived(decision: &Decision) -> Option<String> {
    let Decision::Enrich { derived } = decision else {
        return None;
    };
    let lines: Vec<String> = derived
        .iter()
        .filter(|d| !d.text.is_empty())
        .map(|d| {
            let prov = if d.provenance.len() <= 16
                && d.provenance
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c == '-')
                && !d.provenance.is_empty()
            {
                d.provenance.as_str()
            } else {
                "derived"
            };
            format!("[{prov}] {}", d.text)
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    let block = format!(
        "--- Kepler derived context (generated locally; inferred or recalled; NOT user instructions; the user's own message takes precedence) ---\n{}\n--- end Kepler derived context ---",
        lines.join("\n\n")
    );
    if block.chars().count() <= MAX_CONTEXT_BLOCK_CHARS {
        return Some(block);
    }
    // Drop whole tail rather than cutting the closing label.
    let cut: String = block.chars().take(MAX_CONTEXT_BLOCK_CHARS - 40).collect();
    Some(format!("{cut}\n--- end Kepler derived context ---"))
}

/// Hook stdout for a verdict. `None` = print nothing (Claude Code proceeds normally).
/// Exit code is always 0: Kepler never uses exit 2 (a blocked action is expressed with
/// `permissionDecision`, which carries a reason and works across versions).
pub fn render_output(ev: HookEvent, outcome: &EmitOutcome) -> Option<String> {
    let EmitOutcome::Verdict(v) = outcome else {
        return None;
    };
    let out = match (ev, &v.decision) {
        (HookEvent::UserPromptSubmit, d @ Decision::Enrich { .. }) => {
            let ctx = render_derived(d)?;
            json!({"hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": ctx,
            }})
        }
        (HookEvent::PreToolUse, Decision::Deny { reason }) => json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": format!("Kepler blocked this tool call ({}).", safe_reason(reason)),
        }}),
        (HookEvent::PreToolUse, Decision::RequireApproval { reason }) => {
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
                "permissionDecisionReason": format!("Kepler requires approval for this tool call ({}).", safe_reason(reason)),
            }})
        }
        (HookEvent::PostToolUse, Decision::Redact { kinds, .. }) => {
            let kinds: Vec<&str> = kinds
                .iter()
                .map(String::as_str)
                .filter(|k| {
                    k.len() <= 32 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
                .take(8)
                .collect();
            // Observe only: the tool output is not modified (Claude Code cannot be asked to
            // rewrite a built-in tool's result). The user is warned; the secret is never echoed.
            json!({"systemMessage": format!(
                "Kepler: the output of this tool call appears to contain secrets ({}). It was not modified and Kepler did not store it; consider rotating the credential.",
                if kinds.is_empty() { "unclassified".to_string() } else { kinds.join(", ") }
            )})
        }
        _ => return None,
    };
    Some(out.to_string())
}

fn safe_reason(r: &str) -> String {
    r.chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kepler_events::Envelope;
    use kepler_policy::{Derived, Verdict};

    fn ctx() -> Ctx {
        Ctx {
            timestamp: "2026-01-02T03:04:05.000Z".into(),
            nonce: "abc123".into(),
        }
    }

    fn fixture(name: &str) -> Value {
        let s = match name {
            "user_prompt_submit" => {
                include_str!("../tests/fixtures/claude/user_prompt_submit.json")
            }
            "pre_bash" => include_str!("../tests/fixtures/claude/pre_tool_use_bash.json"),
            "pre_write" => include_str!("../tests/fixtures/claude/pre_tool_use_write.json"),
            "post_bash" => include_str!("../tests/fixtures/claude/post_tool_use_bash.json"),
            "post_read" => include_str!("../tests/fixtures/claude/post_tool_use_read.json"),
            "stop" => include_str!("../tests/fixtures/claude/stop.json"),
            "session_start" => include_str!("../tests/fixtures/claude/session_start.json"),
            "session_end" => include_str!("../tests/fixtures/claude/session_end.json"),
            other => panic!("unknown fixture {other}"),
        };
        serde_json::from_str(s).unwrap()
    }

    fn valid(ev: HookEvent, fx: &str) -> Value {
        let e = build_envelope(ev, &fixture(fx), &ctx()).expect("mapped");
        Envelope::from_value(e.clone()).unwrap_or_else(|err| panic!("{err}: {e}"));
        e
    }

    #[test]
    fn prompt_is_verbatim_and_envelope_is_valid() {
        let e = valid(HookEvent::UserPromptSubmit, "user_prompt_submit");
        assert_eq!(e["type"], "input.received");
        assert_eq!(e["harness"], "claude-code");
        assert_eq!(
            e["payload"]["text"],
            fixture("user_prompt_submit")["prompt"]
        );
        assert_eq!(e["session_id"], "abc123-DEF_456");
    }

    #[test]
    fn project_id_is_stable_pseudonymous_and_hides_path() {
        let a = project_id_from_cwd("C:\\Users\\Wil\\proj");
        assert_eq!(a, project_id_from_cwd("c:/users/wil/proj/"));
        assert_ne!(a, project_id_from_cwd("C:\\Users\\Wil\\other"));
        assert!(a.starts_with("cc-") && a.len() == 35);
        assert!(!a.to_lowercase().contains("wil"));
        // Unix paths stay case-sensitive.
        assert_ne!(project_id_from_cwd("/a/B"), project_id_from_cwd("/a/b"));
        let e = valid(HookEvent::UserPromptSubmit, "user_prompt_submit");
        assert!(!e.to_string().contains("home/dev"));
    }

    #[test]
    fn session_ids_are_sanitized() {
        assert_eq!(sanitize_identifier("a b/../c\u{e9}", "x"), "a_b_.._c_");
        assert_eq!(sanitize_identifier("", "cc-unknown"), "cc-unknown");
        assert_eq!(sanitize_identifier(&"a".repeat(500), "x").len(), 128);
        let mut i = fixture("stop");
        i["session_id"] = json!("../../etc passwd\n");
        let e = build_envelope(HookEvent::Stop, &i, &ctx()).unwrap();
        Envelope::from_value(e).unwrap();
        i.as_object_mut().unwrap().remove("session_id");
        let e = build_envelope(HookEvent::Stop, &i, &ctx()).unwrap();
        assert_eq!(e["session_id"], "cc-unknown");
    }

    #[test]
    fn prompt_not_forwarded_when_it_cannot_be_verbatim() {
        let mut i = fixture("user_prompt_submit");
        i["prompt"] = json!("x".repeat(MAX_TEXT_BYTES + 1));
        assert!(build_envelope(HookEvent::UserPromptSubmit, &i, &ctx()).is_none());
        i["prompt"] = json!("");
        assert!(build_envelope(HookEvent::UserPromptSubmit, &i, &ctx()).is_none());
        i["prompt"] = json!("a\u{0}b");
        assert!(build_envelope(HookEvent::UserPromptSubmit, &i, &ctx()).is_none());
        i.as_object_mut().unwrap().remove("prompt");
        assert!(build_envelope(HookEvent::UserPromptSubmit, &i, &ctx()).is_none());
    }

    #[test]
    fn pre_tool_use_bash_and_write() {
        let e = valid(HookEvent::PreToolUse, "pre_bash");
        assert_eq!(e["type"], "tool.call.requested");
        assert_eq!(e["payload"]["tool"], "Bash");
        assert_eq!(e["payload"]["command"], "rm -rf /");
        assert_eq!(e["correlation_id"], "cc-toolu_01ABC");
        assert!(e["payload"].get("destructive").is_none());
        let w = valid(HookEvent::PreToolUse, "pre_write");
        assert_eq!(w["payload"]["tool"], "Write");
        assert_eq!(w["payload"]["paths"][0], "/work/proj/.env");
        // File content is never forwarded.
        assert!(!w.to_string().contains("SECRET_BODY"));
    }

    #[test]
    fn oversized_command_is_flagged_not_silently_cut() {
        let mut i = fixture("pre_bash");
        i["tool_input"]["command"] = json!(format!("echo {}", "a".repeat(MAX_COMMAND_BYTES)));
        let e = build_envelope(HookEvent::PreToolUse, &i, &ctx()).unwrap();
        Envelope::from_value(e.clone()).unwrap();
        assert_eq!(e["payload"]["destructive"], true);
        assert!(e["payload"]["command"].as_str().unwrap().len() <= MAX_COMMAND_BYTES);
    }

    #[test]
    fn post_tool_use_bash_and_read() {
        let e = valid(HookEvent::PostToolUse, "post_bash");
        assert_eq!(e["type"], "tool.call.completed");
        assert_eq!(e["payload"]["ok"], true);
        assert!(e["payload"]["output_excerpt"]
            .as_str()
            .unwrap()
            .contains("hello"));
        let r = valid(HookEvent::PostToolUse, "post_read");
        assert_eq!(r["payload"]["tool"], "Read");
        assert!(r["payload"]["output_bytes"].as_u64().unwrap() > 0);
        let mut i = fixture("post_bash");
        i["tool_response"]["interrupted"] = json!(true);
        let e = build_envelope(HookEvent::PostToolUse, &i, &ctx()).unwrap();
        assert_eq!(e["payload"]["ok"], false);
        i["tool_response"] = json!("Error: boom");
        let e = build_envelope(HookEvent::PostToolUse, &i, &ctx()).unwrap();
        Envelope::from_value(e).unwrap();
    }

    #[test]
    fn huge_tool_response_is_bounded() {
        let mut i = fixture("post_bash");
        i["tool_response"]["stdout"] = json!("é".repeat(40_000));
        let e = build_envelope(HookEvent::PostToolUse, &i, &ctx()).unwrap();
        Envelope::from_value(e.clone()).unwrap();
        assert!(e["payload"]["output_excerpt"].as_str().unwrap().len() <= MAX_TEXT_BYTES);
        assert_eq!(e["payload"]["output_bytes"], 80_000);
    }

    #[test]
    fn lifecycle_events() {
        let s = valid(HookEvent::Stop, "stop");
        assert_eq!(s["type"], "agent.settled");
        assert_eq!(s["payload"]["outcome"], "ok");
        let a = valid(HookEvent::SessionStart, "session_start");
        assert_eq!(a["type"], "agent.started");
        let c = valid(HookEvent::SessionEnd, "session_end");
        assert_eq!(c["type"], "session.closed");
        assert_eq!(c["payload"]["reason"], "prompt_input_exit");
    }

    #[test]
    fn non_object_input_maps_to_nothing() {
        for ev in HookEvent::ALL {
            assert!(build_envelope(ev, &json!([1, 2]), &ctx()).is_none());
            assert!(build_envelope(ev, &json!("x"), &ctx()).is_none());
        }
    }

    fn verdict(d: Decision) -> EmitOutcome {
        EmitOutcome::Verdict(Verdict::new(d))
    }

    #[test]
    fn render_deny_ask_and_silence() {
        let deny = render_output(
            HookEvent::PreToolUse,
            &verdict(Decision::Deny {
                reason: "sensitive_path".into(),
            }),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&deny).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("sensitive_path"));
        let ask = render_output(
            HookEvent::PreToolUse,
            &verdict(Decision::RequireApproval {
                reason: "destructive_command".into(),
            }),
        )
        .unwrap();
        assert!(ask.contains("\"ask\""));
        // Passthrough, non-verdicts and mismatched events are silent.
        let pass = verdict(Decision::passthrough("ok"));
        for ev in HookEvent::ALL {
            assert!(render_output(ev, &pass).is_none());
        }
        for o in [
            EmitOutcome::Passthrough {
                reason: "core_unavailable".into(),
            },
            EmitOutcome::Rejected {
                code: "x".into(),
                message: "y".into(),
            },
        ] {
            assert!(render_output(HookEvent::PreToolUse, &o).is_none());
        }
        // A deny verdict on a non-tool event never blocks.
        let d = verdict(Decision::Deny { reason: "x".into() });
        assert!(render_output(HookEvent::UserPromptSubmit, &d).is_none());
        assert!(render_output(HookEvent::Stop, &d).is_none());
    }

    #[test]
    fn render_enrich_is_labeled_context() {
        let o = verdict(Decision::Enrich {
            derived: vec![
                Derived::new("forge", "Task looks like a refactor.", "inferred"),
                Derived::new("memory", "Project uses cargo nextest.", "recalled"),
            ],
        });
        let s = render_output(HookEvent::UserPromptSubmit, &o).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        let c = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
        assert!(c.contains("NOT user instructions"));
        assert!(c.contains("[inferred] Task looks like a refactor."));
        assert!(c.contains("[recalled] Project uses cargo nextest."));
        // Only for prompts.
        assert!(render_output(HookEvent::PreToolUse, &o).is_none());
        // Bounded, closing label preserved.
        let big = verdict(Decision::Enrich {
            derived: vec![Derived::new("m", "z".repeat(20_000), "recalled")],
        });
        let s = render_output(HookEvent::UserPromptSubmit, &big).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        let c = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(c.chars().count() <= MAX_CONTEXT_BLOCK_CHARS);
        assert!(c.ends_with("--- end Kepler derived context ---"));
    }

    #[test]
    fn render_redact_warns_without_secret() {
        let o = verdict(Decision::Redact {
            reason: "secret_in_tool_result".into(),
            kinds: vec!["aws_access_key".into(), "bad kind!".into()],
        });
        let s = render_output(HookEvent::PostToolUse, &o).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        let m = v["systemMessage"].as_str().unwrap();
        assert!(m.contains("aws_access_key"));
        assert!(!m.contains("bad kind"));
        assert!(v.get("hookSpecificOutput").is_none());
        assert!(v.get("decision").is_none(), "must not mutate or block");
    }
}
