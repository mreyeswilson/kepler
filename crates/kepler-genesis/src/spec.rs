//! Versioned UI specification (`kepler.genesis.spec/1`) and its strict validator.
//!
//! The model's answer is UNTRUSTED data. [`parse_answer`] accepts only this closed schema:
//! unknown keys, wrong types, over-long strings, out-of-range counts and values outside the
//! enums are rejected (per candidate: one bad candidate does not discard the good ones).
//! Free text is sanitized (control characters and angle brackets removed, `javascript:` stripped)
//! and is later HTML-escaped again by the renderer, which never emits model-provided markup.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub const SPEC_SCHEMA: u32 = 1;
pub const SPEC_SCHEMA_NAME: &str = "kepler.genesis.spec/1";
pub const MAX_ANSWER_BYTES: usize = 48 * 1024;
pub const MAX_CANDIDATES: usize = 3;

const MAX_SCREENS: usize = 4;
const MAX_SECTIONS: usize = 6;
const MAX_COMPONENTS: usize = 10;
const MAX_PLAN: usize = 16;

pub const LAYOUT_KINDS: &[&str] = &[
    "single_column",
    "centered_card",
    "two_column",
    "sidebar_content",
    "split_hero",
    "grid",
    "dashboard",
    "list_detail",
];

pub const COMPONENT_KINDS: &[&str] = &[
    "heading",
    "text",
    "button",
    "link",
    "input",
    "email_input",
    "password_input",
    "textarea",
    "select",
    "checkbox",
    "radio",
    "switch",
    "card",
    "form",
    "list",
    "table",
    "image",
    "icon",
    "nav",
    "tabs",
    "alert",
    "badge",
    "avatar",
    "divider",
    "progress",
    "modal",
];

const BREAKPOINTS: &[&str] = &["mobile", "tablet", "desktop"];
const PLAN_ACTIONS: &[&str] = &["create", "modify"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Layout {
    pub kind: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Component {
    pub kind: String,
    pub label: Option<String>,
    pub library_component: Option<String>,
    pub variant: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    pub id: String,
    pub name: String,
    pub components: Vec<Component>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct States {
    pub empty: String,
    pub loading: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Responsive {
    pub breakpoint: String,
    pub behavior: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Screen {
    pub id: String,
    pub name: String,
    pub purpose: String,
    pub layout: Layout,
    pub sections: Vec<Section>,
    pub states: States,
    pub responsive: Vec<Responsive>,
    pub accessibility: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenRef {
    pub role: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanStep {
    pub path: String,
    pub action: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub name: String,
    pub rationale: String,
    pub goal: String,
    pub audience: Vec<String>,
    pub screens: Vec<Screen>,
    pub tokens_used: Vec<TokenRef>,
    pub implementation_plan: Vec<PlanStep>,
    /// Token names that are not in the project's design tokens (informational).
    pub unknown_tokens: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub candidates: Vec<Candidate>,
    /// One reason per rejected candidate (never echoes model text beyond a field path).
    pub rejected: Vec<String>,
    /// Count of strings that had control characters or markup removed.
    pub sanitized: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecError {
    TooLarge,
    NotJson,
    BadEnvelope(String),
    NoValidCandidate(Vec<String>),
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "model answer larger than {MAX_ANSWER_BYTES} bytes"),
            Self::NotJson => f.write_str("model answer is not a single JSON object"),
            Self::BadEnvelope(m) => write!(f, "invalid answer envelope: {m}"),
            Self::NoValidCandidate(r) => write!(f, "no valid candidate: {}", r.join("; ")),
        }
    }
}

impl std::error::Error for SpecError {}

type R<T> = Result<T, String>;

struct Ctx {
    sanitized: u32,
    known_tokens: BTreeSet<String>,
    unknown: BTreeSet<String>,
}

fn object<'a>(v: &'a Value, path: &str, allowed: &[&str]) -> R<&'a Map<String, Value>> {
    let m = v
        .as_object()
        .ok_or_else(|| format!("{path}: expected an object"))?;
    for k in m.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("{path}: unknown field `{}`", short(k)));
        }
    }
    Ok(m)
}

fn short(s: &str) -> String {
    s.chars().take(24).filter(|c| !c.is_control()).collect()
}

fn array<'a>(
    m: &'a Map<String, Value>,
    key: &str,
    path: &str,
    min: usize,
    max: usize,
) -> R<&'a Vec<Value>> {
    let a = m
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{path}.{key}: expected an array"))?;
    if a.len() < min || a.len() > max {
        return Err(format!(
            "{path}.{key}: expected {min}..={max} items, got {}",
            a.len()
        ));
    }
    Ok(a)
}

fn opt_array<'a>(
    m: &'a Map<String, Value>,
    key: &str,
    path: &str,
    max: usize,
) -> R<Vec<&'a Value>> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) if a.len() <= max => Ok(a.iter().collect()),
        _ => Err(format!(
            "{path}.{key}: expected an array of at most {max} items"
        )),
    }
}

/// Clean free text: control characters become spaces, `<` and `>` are removed, `javascript:`
/// is stripped, whitespace collapsed. Length is checked on the raw input (reject, not truncate).
fn text(c: &mut Ctx, v: Option<&Value>, path: &str, max: usize, required: bool) -> R<String> {
    let s = match v {
        Some(Value::String(s)) => s,
        None | Some(Value::Null) if !required => return Ok(String::new()),
        _ => return Err(format!("{path}: expected a string")),
    };
    if s.chars().count() > max {
        return Err(format!("{path}: longer than {max} characters"));
    }
    let mut changed = false;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_control() {
            changed = true;
            out.push(' ');
        } else if ch == '<' || ch == '>' {
            changed = true;
        } else {
            out.push(ch);
        }
    }
    let mut lower = out.to_ascii_lowercase();
    while let Some(i) = lower.find("javascript:") {
        changed = true;
        out.replace_range(i..i + "javascript:".len(), "");
        lower = out.to_ascii_lowercase();
    }
    if changed {
        c.sanitized += 1;
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if required && out.is_empty() {
        return Err(format!("{path}: must not be empty"));
    }
    Ok(out)
}

fn opt_text(
    c: &mut Ctx,
    m: &Map<String, Value>,
    key: &str,
    path: &str,
    max: usize,
) -> R<Option<String>> {
    let s = text(c, m.get(key), &format!("{path}.{key}"), max, false)?;
    Ok((!s.is_empty()).then_some(s))
}

fn slug(c: &mut Ctx, v: Option<&Value>, path: &str) -> R<String> {
    let s = text(c, v, path, 40, true)?;
    if s.chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        Ok(s)
    } else {
        Err(format!("{path}: expected a lowercase slug (a-z, 0-9, -)"))
    }
}

fn ident(c: &mut Ctx, s: Option<String>, path: &str, max: usize) -> R<Option<String>> {
    let _ = c;
    match s {
        None => Ok(None),
        Some(s)
            if s.len() <= max
                && s.chars().all(|ch| {
                    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ' ')
                }) =>
        {
            Ok(Some(s))
        }
        Some(_) => Err(format!("{path}: unexpected characters")),
    }
}

fn enum_of(c: &mut Ctx, v: Option<&Value>, path: &str, allowed: &[&str]) -> R<String> {
    let s = text(c, v, path, 32, true)?;
    if allowed.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(format!("{path}: value not in the closed list"))
    }
}

fn string_list(
    c: &mut Ctx,
    m: &Map<String, Value>,
    key: &str,
    path: &str,
    max_items: usize,
    max_len: usize,
) -> R<Vec<String>> {
    opt_array(m, key, path, max_items)?
        .into_iter()
        .enumerate()
        .map(|(i, v)| text(c, Some(v), &format!("{path}.{key}[{i}]"), max_len, true))
        .collect()
}

fn plan_path(s: &str, path: &str) -> R<()> {
    let ok = !s.is_empty()
        && s.len() <= 120
        && !s.starts_with('/')
        && !s.starts_with('\\')
        && !s.contains("..")
        && !s.contains(':')
        && !s.contains('\\')
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '.' | '/' | '-' | '[' | ']' | '(' | ')' | '@')
        });
    if ok {
        Ok(())
    } else {
        Err(format!("{path}: must be a safe relative path"))
    }
}

fn component(c: &mut Ctx, v: &Value, path: &str) -> R<Component> {
    let m = object(
        v,
        path,
        &["kind", "label", "library_component", "variant", "notes"],
    )?;
    let lib = opt_text(c, m, "library_component", path, 60)?;
    let variant = opt_text(c, m, "variant", path, 30)?;
    Ok(Component {
        kind: enum_of(c, m.get("kind"), &format!("{path}.kind"), COMPONENT_KINDS)?,
        label: opt_text(c, m, "label", path, 80)?,
        library_component: ident(c, lib, &format!("{path}.library_component"), 60)?,
        variant: ident(c, variant, &format!("{path}.variant"), 30)?,
        notes: opt_text(c, m, "notes", path, 160)?,
    })
}

fn section(c: &mut Ctx, v: &Value, path: &str) -> R<Section> {
    let m = object(v, path, &["id", "name", "components"])?;
    let comps = array(m, "components", path, 1, MAX_COMPONENTS)?;
    Ok(Section {
        id: slug(c, m.get("id"), &format!("{path}.id"))?,
        name: text(c, m.get("name"), &format!("{path}.name"), 60, true)?,
        components: comps
            .iter()
            .enumerate()
            .map(|(i, v)| component(c, v, &format!("{path}.components[{i}]")))
            .collect::<R<_>>()?,
    })
}

fn screen(c: &mut Ctx, v: &Value, path: &str) -> R<Screen> {
    let m = object(
        v,
        path,
        &[
            "id",
            "name",
            "purpose",
            "layout",
            "sections",
            "states",
            "responsive",
            "accessibility",
        ],
    )?;
    let lp = format!("{path}.layout");
    let lm = object(
        m.get("layout").ok_or(format!("{lp}: missing"))?,
        &lp,
        &["kind", "description"],
    )?;
    let sp = format!("{path}.states");
    let sm = object(
        m.get("states").ok_or(format!("{sp}: missing"))?,
        &sp,
        &["empty", "loading", "error"],
    )?;
    let secs = array(m, "sections", path, 1, MAX_SECTIONS)?;
    let mut responsive = Vec::new();
    for (i, v) in opt_array(m, "responsive", path, 3)?.into_iter().enumerate() {
        let rp = format!("{path}.responsive[{i}]");
        let rm = object(v, &rp, &["breakpoint", "behavior"])?;
        responsive.push(Responsive {
            breakpoint: enum_of(
                c,
                rm.get("breakpoint"),
                &format!("{rp}.breakpoint"),
                BREAKPOINTS,
            )?,
            behavior: text(c, rm.get("behavior"), &format!("{rp}.behavior"), 200, true)?,
        });
    }
    Ok(Screen {
        id: slug(c, m.get("id"), &format!("{path}.id"))?,
        name: text(c, m.get("name"), &format!("{path}.name"), 60, true)?,
        purpose: text(c, m.get("purpose"), &format!("{path}.purpose"), 240, true)?,
        layout: Layout {
            kind: enum_of(c, lm.get("kind"), &format!("{lp}.kind"), LAYOUT_KINDS)?,
            description: text(
                c,
                lm.get("description"),
                &format!("{lp}.description"),
                300,
                true,
            )?,
        },
        sections: secs
            .iter()
            .enumerate()
            .map(|(i, v)| section(c, v, &format!("{path}.sections[{i}]")))
            .collect::<R<_>>()?,
        states: States {
            empty: text(c, sm.get("empty"), &format!("{sp}.empty"), 200, true)?,
            loading: text(c, sm.get("loading"), &format!("{sp}.loading"), 200, true)?,
            error: text(c, sm.get("error"), &format!("{sp}.error"), 200, true)?,
        },
        responsive,
        accessibility: string_list(c, m, "accessibility", path, 6, 160)?,
    })
}

fn candidate(c: &mut Ctx, v: &Value, path: &str) -> R<Candidate> {
    let m = object(
        v,
        path,
        &[
            "name",
            "rationale",
            "goal",
            "audience",
            "screens",
            "tokens_used",
            "implementation_plan",
        ],
    )?;
    let screens = array(m, "screens", path, 1, MAX_SCREENS)?;
    let mut tokens_used = Vec::new();
    for (i, v) in opt_array(m, "tokens_used", path, 12)?
        .into_iter()
        .enumerate()
    {
        let tp = format!("{path}.tokens_used[{i}]");
        let tm = object(v, &tp, &["role", "token"])?;
        let token = text(c, tm.get("token"), &format!("{tp}.token"), 48, true)?;
        let token = ident(c, Some(token), &format!("{tp}.token"), 48)?.unwrap_or_default();
        if !c.known_tokens.contains(&token) {
            c.unknown.insert(token.clone());
        }
        tokens_used.push(TokenRef {
            role: text(c, tm.get("role"), &format!("{tp}.role"), 40, true)?,
            token,
        });
    }
    let mut plan = Vec::new();
    for (i, v) in array(m, "implementation_plan", path, 1, MAX_PLAN)?
        .iter()
        .enumerate()
    {
        let pp = format!("{path}.implementation_plan[{i}]");
        let pm = object(v, &pp, &["path", "action", "description"])?;
        let p = text(c, pm.get("path"), &format!("{pp}.path"), 120, true)?;
        plan_path(&p, &format!("{pp}.path"))?;
        plan.push(PlanStep {
            path: p,
            action: enum_of(c, pm.get("action"), &format!("{pp}.action"), PLAN_ACTIONS)?,
            description: text(
                c,
                pm.get("description"),
                &format!("{pp}.description"),
                200,
                true,
            )?,
        });
    }
    let cand = Candidate {
        name: text(c, m.get("name"), &format!("{path}.name"), 60, true)?,
        rationale: text(
            c,
            m.get("rationale"),
            &format!("{path}.rationale"),
            300,
            true,
        )?,
        goal: text(c, m.get("goal"), &format!("{path}.goal"), 300, true)?,
        audience: string_list(c, m, "audience", path, 4, 100)?,
        screens: screens
            .iter()
            .enumerate()
            .map(|(i, v)| screen(c, v, &format!("{path}.screens[{i}]")))
            .collect::<R<_>>()?,
        tokens_used,
        implementation_plan: plan,
        unknown_tokens: Vec::new(),
    };
    Ok(cand)
}

/// Extract the single JSON object from the model's text: either the whole text or one fenced
/// block. Anything else around it is rejected.
fn json_text(raw: &str) -> Option<&str> {
    let t = raw.trim();
    if t.starts_with('{') && t.ends_with('}') {
        return Some(t);
    }
    let inner = t
        .strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))?;
    let inner = inner.trim().strip_suffix("```")?.trim();
    (inner.starts_with('{') && inner.ends_with('}')).then_some(inner)
}

/// Validate a model answer. `known_tokens` are the project's design-token names.
pub fn parse_answer(
    raw: &str,
    known_tokens: &BTreeSet<String>,
    wanted: usize,
) -> Result<Parsed, SpecError> {
    if raw.len() > MAX_ANSWER_BYTES {
        return Err(SpecError::TooLarge);
    }
    let text = json_text(raw).ok_or(SpecError::NotJson)?;
    let v: Value = serde_json::from_str(text).map_err(|_| SpecError::NotJson)?;
    let m =
        object(&v, "answer", &["schema_version", "candidates"]).map_err(SpecError::BadEnvelope)?;
    if m.get("schema_version").and_then(Value::as_u64) != Some(u64::from(SPEC_SCHEMA)) {
        return Err(SpecError::BadEnvelope("schema_version must be 1".into()));
    }
    let cands = m
        .get("candidates")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty() && a.len() <= MAX_CANDIDATES)
        .ok_or_else(|| {
            SpecError::BadEnvelope(format!("candidates must hold 1..={MAX_CANDIDATES} items"))
        })?;
    let mut ctx = Ctx {
        sanitized: 0,
        known_tokens: known_tokens.clone(),
        unknown: BTreeSet::new(),
    };
    let mut out = Vec::new();
    let mut rejected = Vec::new();
    for (i, v) in cands.iter().enumerate() {
        ctx.unknown.clear();
        match candidate(&mut ctx, v, &format!("candidates[{i}]")) {
            Ok(mut c) => {
                c.unknown_tokens = ctx.unknown.iter().cloned().collect();
                if out.len() < wanted.clamp(1, MAX_CANDIDATES) {
                    out.push(c);
                }
            }
            Err(e) => rejected.push(e),
        }
    }
    if out.is_empty() {
        return Err(SpecError::NoValidCandidate(rejected));
    }
    Ok(Parsed {
        candidates: out,
        rejected,
        sanitized: ctx.sanitized,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    pub fn good_candidate() -> Value {
        json!({
            "name": "Centered card",
            "rationale": "Single focused card keeps the login flow short.",
            "goal": "Let people sign in, register or recover a password.",
            "audience": ["returning customers", "new visitors"],
            "screens": [{
                "id": "login", "name": "Sign in", "purpose": "Authenticate an existing user.",
                "layout": {"kind": "centered_card", "description": "Card centered on a muted surface; heading, form, secondary links."},
                "sections": [
                    {"id": "form", "name": "Credentials", "components": [
                        {"kind": "heading", "label": "Welcome back"},
                        {"kind": "email_input", "label": "Email", "library_component": "Input"},
                        {"kind": "password_input", "label": "Password", "library_component": "Input", "notes": "Show/hide toggle"},
                        {"kind": "button", "label": "Sign in", "library_component": "Button", "variant": "default"}
                    ]},
                    {"id": "links", "name": "Secondary", "components": [
                        {"kind": "link", "label": "Forgot your password?"},
                        {"kind": "link", "label": "Create an account"}
                    ]}
                ],
                "states": {"empty": "Fields show placeholders.", "loading": "Button shows a spinner and is disabled.", "error": "Inline alert above the form with the reason."},
                "responsive": [{"breakpoint": "mobile", "behavior": "Card becomes full width with 16px gutters."}],
                "accessibility": ["Labels bound to inputs", "Error alert uses role=alert"]
            }],
            "tokens_used": [{"role": "primary action", "token": "brand"}, {"role": "surface", "token": "surface"}],
            "implementation_plan": [
                {"path": "src/pages/Login.tsx", "action": "create", "description": "Login screen using Card, Input and Button."},
                {"path": "src/App.tsx", "action": "modify", "description": "Add the /login route."}
            ]
        })
    }

    pub fn answer(cands: Vec<Value>) -> String {
        json!({"schema_version": 1, "candidates": cands}).to_string()
    }

    fn known() -> BTreeSet<String> {
        ["brand", "surface"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn valid_answer_parses_and_flags_unknown_tokens() {
        let mut c = good_candidate();
        c["tokens_used"][1]["token"] = json!("invented-color");
        let p = parse_answer(&answer(vec![c]), &known(), 2).unwrap();
        assert_eq!(p.candidates.len(), 1);
        assert_eq!(p.candidates[0].screens[0].sections.len(), 2);
        assert_eq!(
            p.candidates[0].unknown_tokens,
            vec!["invented-color".to_string()]
        );
        assert!(p.rejected.is_empty() && p.sanitized == 0);
    }

    #[test]
    fn fenced_json_is_accepted_but_prose_around_it_is_not() {
        let a = answer(vec![good_candidate()]);
        assert!(parse_answer(&format!("```json\n{a}\n```"), &known(), 1).is_ok());
        assert_eq!(
            parse_answer(&format!("Sure! {a}"), &known(), 1),
            Err(SpecError::NotJson)
        );
        assert_eq!(
            parse_answer("I cannot do that", &known(), 1),
            Err(SpecError::NotJson)
        );
        assert_eq!(parse_answer("", &known(), 1), Err(SpecError::NotJson));
    }

    #[test]
    fn extra_fields_are_rejected_at_every_level() {
        for path in [
            "",
            "screens/0",
            "screens/0/layout",
            "screens/0/sections/0",
            "screens/0/sections/0/components/0",
            "tokens_used/0",
            "implementation_plan/0",
            "screens/0/states",
        ] {
            let mut c = good_candidate();
            let target = if path.is_empty() {
                &mut c
            } else {
                c.pointer_mut(&format!("/{path}")).unwrap()
            };
            target
                .as_object_mut()
                .unwrap()
                .insert("run".into(), json!("rm -rf /"));
            let r = parse_answer(&answer(vec![c]), &known(), 1);
            assert!(
                matches!(r, Err(SpecError::NoValidCandidate(ref e)) if e[0].contains("unknown field")),
                "{path}: {r:?}"
            );
        }
        let a =
            json!({"schema_version": 1, "candidates": [good_candidate()], "extra": 1}).to_string();
        assert!(matches!(
            parse_answer(&a, &known(), 1),
            Err(SpecError::BadEnvelope(_))
        ));
    }

    #[test]
    fn markup_and_script_text_is_sanitized_not_trusted() {
        let mut c = good_candidate();
        c["name"] = json!("<script>alert(1)</script>Evil");
        c["screens"][0]["sections"][0]["components"][0]["label"] =
            json!("<img src=x onerror=alert(1)>Hi\u{0}\u{7}");
        c["screens"][0]["purpose"] = json!("click JavaScript:alert(1)");
        let p = parse_answer(&answer(vec![c]), &known(), 1).unwrap();
        let s = serde_json::to_string(&p.candidates[0]).unwrap();
        assert!(
            !s.contains('<') && !s.contains('>') && !s.to_lowercase().contains("javascript:"),
            "{s}"
        );
        assert!(p.sanitized >= 3);
    }

    #[test]
    fn enums_lengths_counts_and_types_are_enforced() {
        let cases: Vec<(&str, Value)> = vec![
            ("/screens/0/layout/kind", json!("freeform")),
            ("/screens/0/sections/0/components/0/kind", json!("script")),
            ("/implementation_plan/0/action", json!("delete")),
            ("/screens/0/responsive/0/breakpoint", json!("4k")),
            ("/name", json!("x".repeat(61))),
            ("/goal", json!("x".repeat(10_000))),
            ("/screens/0/id", json!("Not A Slug!")),
            ("/implementation_plan/0/path", json!("../../etc/passwd")),
            ("/implementation_plan/1/path", json!("C:/Windows/x")),
            ("/implementation_plan/1/path", json!("/etc/passwd")),
            ("/screens", json!([])),
            ("/name", json!(42)),
            (
                "/screens/0/sections/0/components/1/library_component",
                json!("Input\"><script>"),
            ),
        ];
        for (ptr, bad) in cases {
            let mut c = good_candidate();
            *c.pointer_mut(ptr).unwrap() = bad;
            assert!(
                parse_answer(&answer(vec![c]), &known(), 1).is_err(),
                "{ptr}"
            );
        }
        let mut c = good_candidate();
        let screen = c["screens"][0].clone();
        c["screens"] = json!(vec![screen; 5]);
        assert!(parse_answer(&answer(vec![c]), &known(), 1).is_err());
    }

    #[test]
    fn one_bad_candidate_does_not_discard_the_good_one() {
        let mut bad = good_candidate();
        bad["screens"][0]["layout"]["kind"] = json!("nope");
        let p = parse_answer(&answer(vec![bad, good_candidate()]), &known(), 2).unwrap();
        assert_eq!(p.candidates.len(), 1);
        assert_eq!(p.rejected.len(), 1);
        assert!(
            !p.rejected[0].contains("nope"),
            "rejections must not echo model text"
        );
    }

    #[test]
    fn oversize_answers_and_too_many_candidates_are_rejected() {
        assert_eq!(
            parse_answer(&"x".repeat(MAX_ANSWER_BYTES + 1), &known(), 1),
            Err(SpecError::TooLarge)
        );
        let many = answer(vec![good_candidate(); 4]);
        assert!(matches!(
            parse_answer(&many, &known(), 1),
            Err(SpecError::BadEnvelope(_))
        ));
        // never more candidates than asked for
        let p = parse_answer(&answer(vec![good_candidate(); 3]), &known(), 2).unwrap();
        assert_eq!(p.candidates.len(), 2);
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let a = json!({"schema_version": 2, "candidates": [good_candidate()]}).to_string();
        assert!(matches!(
            parse_answer(&a, &known(), 1),
            Err(SpecError::BadEnvelope(_))
        ));
    }
}
