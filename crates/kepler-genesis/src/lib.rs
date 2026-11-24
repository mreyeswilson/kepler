//! Kepler Genesis (ADR-0027): an opt-in generator of UI *specifications* and static previews.
//!
//! What it is: a deterministic project inspection (stack, design tokens, component library), one
//! bounded call to the user's own `claude -p` that returns a strict JSON specification with 1-3
//! candidates, a validator that treats that answer as untrusted data, a template renderer that
//! turns each candidate into a self-contained HTML wireframe using the project's own tokens, an
//! optional PNG capture, and a small bounded store under the Kepler data directory.
//!
//! What it is not: a visual designer, a code generator or a background service. Nothing here
//! runs unless the user (or the agent, through an explicit tool call) asks for it.

pub mod capture;
pub mod inspect;
pub mod model;
pub mod render;
pub mod spec;
pub mod store;

use inspect::ProjectContext;
use model::{DesignModel, ModelError};
use serde_json::{json, Value};
use spec::SpecError;
use std::path::{Path, PathBuf};
use std::time::Instant;
use store::{Store, StoreError};

#[derive(Debug)]
pub enum GenesisError {
    /// Called from inside a Kepler-spawned model child (recursion guard).
    InChild,
    EmptyBrief,
    Inspect(inspect::InspectError),
    Model(ModelError),
    Spec(SpecError),
    Store(StoreError),
}

impl std::fmt::Display for GenesisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InChild => {
                f.write_str("refusing to run Genesis from inside a Kepler model child")
            }
            Self::EmptyBrief => f.write_str("the brief is empty"),
            Self::Inspect(e) => e.fmt(f),
            Self::Model(e) => e.fmt(f),
            Self::Spec(e) => write!(f, "the model answer was rejected: {e}"),
            Self::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for GenesisError {}

impl From<StoreError> for GenesisError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

#[derive(Debug, Clone)]
pub struct DesignOptions {
    pub brief: String,
    pub project: PathBuf,
    /// 1..=3
    pub candidates: usize,
    /// Try to capture a PNG per candidate (best effort).
    pub capture: bool,
    /// Ignore a previously saved identical design.
    pub fresh: bool,
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Same redacted brief + same project summary + same model + same count => same key.
pub fn cache_key(brief: &str, ctx: &ProjectContext, model: &str, n: usize) -> String {
    let norm = brief
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let blob = format!("{norm}\u{1}{}\u{1}{model}\u{1}{n}", ctx.prompt_json());
    format!("{:016x}", fnv1a(blob.as_bytes()))
}

pub fn inspect_project(project: &Path) -> Result<ProjectContext, GenesisError> {
    inspect::inspect(project).map_err(GenesisError::Inspect)
}

/// Run the whole pipeline and persist the result. Returns the saved design as JSON (see
/// [`outcome_json`]).
pub fn design(
    home: &Path,
    opts: &DesignOptions,
    model: &dyn DesignModel,
) -> Result<Value, GenesisError> {
    if model::in_child() {
        return Err(GenesisError::InChild);
    }
    let brief = model::prepare_brief(&opts.brief);
    if brief.is_empty() {
        return Err(GenesisError::EmptyBrief);
    }
    let n = opts.candidates.clamp(1, spec::MAX_CANDIDATES);
    let ctx = inspect_project(&opts.project)?;
    let store = Store::new(home);
    let key = cache_key(&brief, &ctx, &model.name(), n);
    if !opts.fresh {
        if let Some(id) = store.find_by_key(&key) {
            if let Ok(rec) = store.read(&id) {
                return Ok(outcome_json(&store, &id, &rec, true));
            }
        }
    }

    let started = Instant::now();
    let reply = model
        .ask(
            &model::system_prompt(),
            &model::user_message(&brief, &ctx, n),
        )
        .map_err(GenesisError::Model)?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let parsed =
        spec::parse_answer(&reply.text, &ctx.tokens.names(), n).map_err(GenesisError::Spec)?;

    let (id, dir) = store.alloc()?;
    let finish = || -> Result<Value, GenesisError> {
        let mut htmls = Vec::new();
        let mut pngs = Vec::new();
        for (i, c) in parsed.candidates.iter().enumerate() {
            let p = dir.join(format!("preview-{}.html", i + 1));
            std::fs::write(&p, render::render_candidate(c, &ctx, &brief))
                .map_err(|e| GenesisError::Store(e.into()))?;
            htmls.push(p);
            pngs.push(dir.join(format!("preview-{}.png", i + 1)));
        }
        let report = if opts.capture {
            let r = capture::capture(home, &dir, &htmls, &pngs, &dir.join(".scratch"));
            let _ = std::fs::remove_dir_all(dir.join(".scratch"));
            r
        } else {
            capture::CaptureReport {
                pngs: vec![None; htmls.len()],
                ..Default::default()
            }
        };
        let previews: Vec<Value> = (0..htmls.len())
            .map(|i| {
                json!({
                    "index": i + 1,
                    "html": format!("preview-{}.html", i + 1),
                    "png": report.pngs[i].as_ref().map(|_| format!("preview-{}.png", i + 1)),
                })
            })
            .collect();
        let rec = store::record(
            &id,
            json!({
                "brief": brief,
                "project_name": ctx.project_name,
                "model": model.name(),
                "cache_key": key,
                "context": ctx,
                "candidates": parsed.candidates,
                "previews": previews,
                "rejected": parsed.rejected,
                "sanitized_strings": parsed.sanitized,
                "meta": {
                    "latency_ms": latency_ms,
                    "input_tokens": reply.input_tokens,
                    "output_tokens": reply.output_tokens,
                    "cost_usd": reply.cost_usd,
                    "capture_method": report.method,
                    "capture_notes": report.notes,
                },
            }),
        );
        store.write_record(&dir, &rec)?;
        store.prune(store::MAX_DESIGNS, store::MAX_STORE_BYTES, &id);
        Ok(outcome_json(&store, &id, &rec, false))
    };
    finish().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&dir);
    })
}

/// Bounded summary of a saved design: names, plan paths and preview locations, not the full spec.
pub fn outcome_json(store: &Store, id: &str, rec: &Value, cached: bool) -> Value {
    let dir = store.root().join(id);
    let path = |f: &str| dir.join(f).display().to_string();
    let previews = rec
        .get("previews")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let cands: Vec<Value> = rec
        .get("candidates")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, c)| {
                    let screens: Vec<&str> = c["screens"]
                        .as_array()
                        .map(|s| s.iter().filter_map(|x| x["name"].as_str()).collect())
                        .unwrap_or_default();
                    let comps: usize = c["screens"]
                        .as_array()
                        .map(|s| {
                            s.iter()
                                .flat_map(|x| x["sections"].as_array().into_iter().flatten())
                                .map(|sec| sec["components"].as_array().map_or(0, Vec::len))
                                .sum()
                        })
                        .unwrap_or(0);
                    let files: Vec<&str> = c["implementation_plan"]
                        .as_array()
                        .map(|p| p.iter().filter_map(|x| x["path"].as_str()).collect())
                        .unwrap_or_default();
                    let pv = previews.get(i);
                    json!({
                        "index": i + 1,
                        "name": c["name"],
                        "rationale": c["rationale"],
                        "screens": screens,
                        "components": comps,
                        "planned_files": files,
                        "unknown_tokens": c["unknown_tokens"],
                        "preview_html": pv.and_then(|p| p["html"].as_str()).map(path),
                        "preview_png": pv.and_then(|p| p["png"].as_str()).map(path),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let meta = &rec["meta"];
    json!({
        "id": id,
        "directory": dir.display().to_string(),
        "cached": cached,
        "brief": rec["brief"],
        "model": rec["model"],
        "latency_ms": meta["latency_ms"],
        "usage": {"input_tokens": meta["input_tokens"], "output_tokens": meta["output_tokens"], "cost_usd": meta["cost_usd"]},
        "capture": {"method": meta["capture_method"], "notes": meta["capture_notes"]},
        "candidates": cands,
        "rejected_candidates": rec["rejected"],
        "context_warnings": rec["context"]["warnings"],
        "note": "Specifications and wireframe previews generated by a model from the brief and a summary of the project's stack and tokens. They are proposals, not final design or code."
    })
}

/// One candidate's full specification (for the coding agent that will implement it).
pub fn show_candidate(home: &Path, id: &str, candidate: usize) -> Result<Value, GenesisError> {
    let store = Store::new(home);
    let rec = store.read(id)?;
    let cands = rec
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let c = cands.get(candidate.saturating_sub(1)).ok_or_else(|| {
        GenesisError::Store(StoreError(format!(
            "design `{id}` has {} candidate(s)",
            cands.len()
        )))
    })?;
    let dir = store.root().join(id);
    Ok(json!({
        "id": id,
        "candidate": candidate,
        "brief": rec["brief"],
        "schema": spec::SPEC_SCHEMA_NAME,
        "spec": c,
        "preview_html": dir.join(format!("preview-{candidate}.html")).display().to_string(),
        "preview_png": dir.join(format!("preview-{candidate}.png")).is_file().then(|| dir.join(format!("preview-{candidate}.png")).display().to_string()),
        "project_tokens_known": rec["context"]["tokens"],
    }))
}

pub fn show_outcome(home: &Path, id: &str) -> Result<Value, GenesisError> {
    let store = Store::new(home);
    let rec = store.read(id)?;
    Ok(outcome_json(&store, id, &rec, false))
}

pub fn list_json(home: &Path) -> Value {
    let store = Store::new(home);
    let items: Vec<Value> = store
        .list()
        .into_iter()
        .map(|s| {
            json!({
                "id": s.id, "created_secs": s.created_secs, "brief": s.brief,
                "candidates": s.candidates, "model": s.model, "previews": s.previews, "png_previews": s.pngs,
            })
        })
        .collect();
    json!({"directory": store.root().display().to_string(), "designs": items, "retention": {"max_designs": store::MAX_DESIGNS}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use inspect::tests::{react_tailwind, temp_dir};
    use model::{DesignModel, ModelReply};
    use spec::tests::{answer, good_candidate};
    use std::cell::{Cell, RefCell};

    struct Fake {
        reply: RefCell<Result<String, ModelError>>,
        calls: Cell<u32>,
        seen: RefCell<Vec<(String, String)>>,
    }

    impl Fake {
        fn ok(text: String) -> Self {
            Self {
                reply: RefCell::new(Ok(text)),
                calls: Cell::new(0),
                seen: RefCell::new(vec![]),
            }
        }
        fn err(e: ModelError) -> Self {
            Self {
                reply: RefCell::new(Err(e)),
                calls: Cell::new(0),
                seen: RefCell::new(vec![]),
            }
        }
    }

    impl DesignModel for Fake {
        fn name(&self) -> String {
            "fake".into()
        }
        fn ask(&self, system: &str, user: &str) -> Result<ModelReply, ModelError> {
            self.calls.set(self.calls.get() + 1);
            self.seen.borrow_mut().push((system.into(), user.into()));
            self.reply.borrow().clone().map(|text| ModelReply {
                text,
                input_tokens: 10,
                output_tokens: 20,
                cost_usd: 0.001,
            })
        }
    }

    fn opts(project: PathBuf) -> DesignOptions {
        DesignOptions {
            brief: "pantalla de login".into(),
            project,
            candidates: 2,
            capture: false,
            fresh: false,
        }
    }

    #[test]
    fn happy_path_saves_spec_and_previews_and_caches() {
        let (home, proj) = (temp_dir("home"), react_tailwind());
        let m = Fake::ok(answer(vec![good_candidate(), good_candidate()]));
        let out = design(&home, &opts(proj.clone()), &m).unwrap();
        assert_eq!(out["cached"], false);
        assert_eq!(out["candidates"].as_array().unwrap().len(), 2);
        let id = out["id"].as_str().unwrap().to_string();
        let dir = home.join("genesis").join(&id);
        assert!(
            dir.join("spec.json").is_file()
                && dir.join("preview-1.html").is_file()
                && dir.join("preview-2.html").is_file()
        );
        assert!(!dir.join("preview-1.png").exists());
        assert!(std::fs::read_to_string(dir.join("preview-1.html"))
            .unwrap()
            .contains("--primary:#4f46e5"));
        // The project repo is untouched.
        assert!(!proj.join("genesis").exists());
        // What the model saw: brief + stack summary, never secrets or file contents.
        {
            let seen = m.seen.borrow();
            assert!(
                seen[0].1.contains("React 18")
                    && seen[0].1.contains("#4f46e5")
                    && !seen[0].1.contains("hunter2")
            );
        }
        // Same request again: served from the store, no second model call.
        let again = design(&home, &opts(proj.clone()), &m).unwrap();
        assert_eq!(again["cached"], true);
        assert_eq!(again["id"], out["id"]);
        assert_eq!(m.calls.get(), 1);
        // --fresh forces a new call and a new design.
        let mut o = opts(proj.clone());
        o.fresh = true;
        let fresh = design(&home, &o, &m).unwrap();
        assert_eq!(m.calls.get(), 2);
        assert_ne!(fresh["id"], out["id"]);
        let spec = show_candidate(&home, &id, 2).unwrap();
        assert_eq!(spec["spec"]["name"], "Centered card");
        assert!(show_candidate(&home, &id, 3).is_err());
        assert_eq!(list_json(&home)["designs"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(proj);
    }

    #[test]
    fn failures_leave_nothing_behind_and_never_panic() {
        let (home, proj) = (temp_dir("home-f"), react_tailwind());
        for m in [
            Fake::ok("Sorry, I cannot do that.".into()),
            Fake::ok("{\"schema_version\":1,\"candidates\":[{\"evil\":true}]}".into()),
            Fake::err(ModelError::Timeout(5)),
            Fake::err(ModelError::Unavailable("nope".into())),
        ] {
            assert!(design(&home, &opts(proj.clone()), &m).is_err());
        }
        assert!(Store::new(&home).list().is_empty());
        let leftover = std::fs::read_dir(home.join("genesis"))
            .map(|r| r.count())
            .unwrap_or(0);
        assert_eq!(leftover, 0, "failed designs must not leave directories");
        let mut o = opts(proj.clone());
        o.brief = "   ".into();
        assert!(matches!(
            design(&home, &o, &Fake::ok(String::new())),
            Err(GenesisError::EmptyBrief)
        ));
        o.brief = "x".into();
        o.project = PathBuf::from("/no/such/dir");
        assert!(matches!(
            design(&home, &o, &Fake::ok(String::new())),
            Err(GenesisError::Inspect(_))
        ));
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(proj);
    }

    #[test]
    fn secrets_in_the_brief_never_reach_the_model_or_the_disk() {
        let (home, proj) = (temp_dir("home-s"), react_tailwind());
        let m = Fake::ok(answer(vec![good_candidate()]));
        let mut o = opts(proj.clone());
        o.brief =
            "login con API key sk-ant-api03-ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ y token".into();
        let out = design(&home, &o, &m).unwrap();
        assert!(!m.seen.borrow()[0].1.contains("ZZZZZZZZZZ"));
        let rec = std::fs::read_to_string(
            Path::new(out["directory"].as_str().unwrap()).join("spec.json"),
        )
        .unwrap();
        assert!(!rec.contains("ZZZZZZZZZZ"));
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(proj);
    }

    #[test]
    fn retention_is_enforced_when_saving() {
        let (home, proj) = (temp_dir("home-r"), react_tailwind());
        let m = Fake::ok(answer(vec![good_candidate()]));
        for i in 0..(store::MAX_DESIGNS + 3) {
            let mut o = opts(proj.clone());
            o.brief = format!("brief {i}");
            design(&home, &o, &m).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(Store::new(&home).list().len(), store::MAX_DESIGNS);
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(proj);
    }

    #[test]
    fn cache_key_depends_on_brief_project_model_and_count() {
        let c = ProjectContext::default();
        let k = cache_key("Login  page", &c, "m", 2);
        assert_eq!(k, cache_key("login page", &c, "m", 2));
        assert_ne!(k, cache_key("login page", &c, "m", 3));
        assert_ne!(k, cache_key("login page", &c, "other", 2));
        let mut c2 = c.clone();
        c2.frameworks.push("React".into());
        assert_ne!(k, cache_key("login page", &c2, "m", 2));
    }
}
