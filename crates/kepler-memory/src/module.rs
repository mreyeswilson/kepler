use crate::service::{Ctx, MemoryService};
use kepler_events::{ContextRequested, Envelope, Outcome, Payload};
use kepler_forge::memory_directive;
use kepler_policy::{Advisory, Enrichment, InputClass, ModuleError, OptionalModule};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MAX_SESSIONS_TRACKED: usize = 128;
const MAX_FILES_TRACKED: usize = 16;

/// What Kepler has observed in one session (all bounded, in memory only).
#[derive(Default)]
struct SessionState {
    branch: Option<String>,
    /// Last requested command: (tool, command). Tool calls are sequential per session.
    last_command: Option<(String, String)>,
    files: Vec<String>,
}

/// Memory as a pipeline module (after Forge, before Guardian).
///
/// Besides explicit `memory.candidate` events it derives low-trust candidates from what it
/// *observed* (ADR-0021): verification commands that succeeded and, per branch, the files an
/// accepted task touched. They persist only after an `ok` settlement, like every candidate.
pub struct MemoryModule {
    service: Arc<MemoryService>,
    state: Mutex<HashMap<String, SessionState>>,
}

/// A command that proves how to build/test/lint a project, if `cmd` is one (single simple
/// command: no chaining, pipes, redirects or substitutions).
pub fn verification_command(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    if c.is_empty() || c.len() > 160 || c.contains(['\n', ';', '|', '&', '>', '<', '$', '`', '(']) {
        return None;
    }
    let lower = c.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let first = *words.first()?;
    let second = words.get(1).copied().unwrap_or("");
    let third = words.get(2).copied().unwrap_or("");
    let ok = match first {
        "cargo" => matches!(
            second,
            "test" | "build" | "clippy" | "check" | "fmt" | "nextest"
        ),
        "npm" | "pnpm" | "yarn" | "bun" => {
            matches!(second, "test" | "t")
                || (second == "run" && matches!(third, "test" | "build" | "lint" | "typecheck"))
        }
        "pytest" | "tox" | "ctest" | "tsc" | "eslint" | "ruff" | "mypy" | "rspec" => true,
        "python" | "python3" | "py" => second == "-m" && matches!(third, "pytest" | "unittest"),
        "go" => matches!(second, "test" | "build" | "vet"),
        "dotnet" => matches!(second, "test" | "build"),
        "mvn" | "gradle" | "gradlew" | "./gradlew" => {
            matches!(second, "test" | "build" | "verify" | "check")
        }
        "make" => matches!(second, "test" | "check" | "build" | "lint"),
        _ => false,
    };
    ok.then(|| c.split_whitespace().collect::<Vec<_>>().join(" "))
}

impl MemoryModule {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self {
            service,
            state: Mutex::new(HashMap::new()),
        }
    }

    fn with_state<R>(&self, session: &str, f: impl FnOnce(&mut SessionState) -> R) -> R {
        let mut map = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !map.contains_key(session) && map.len() >= MAX_SESSIONS_TRACKED {
            if let Some(k) = map.keys().next().cloned() {
                map.remove(&k);
            }
        }
        f(map.entry(session.to_string()).or_default())
    }

    fn ctx(&self, event: &Envelope) -> Ctx {
        let announced = match &event.payload {
            Payload::InputReceived(p) => p.branch.clone(),
            Payload::AgentStarted(p) => p.branch.clone(),
            _ => None,
        }
        .filter(|b| !b.trim().is_empty());
        let branch = self.with_state(&event.session_id, |s| {
            if announced.is_some() {
                s.branch = announced;
            }
            s.branch.clone()
        });
        Ctx {
            project_id: event.project_id.clone(),
            session_id: event.session_id.clone(),
            branch,
        }
    }

    fn recall(&self, ctx: &Ctx, text: &str, budget: u32) -> Result<Vec<Enrichment>, ModuleError> {
        let r = self
            .service
            .retrieve(ctx, text, budget)
            .map_err(|e| ModuleError(format!("retrieval failed: {e}")))?;
        Ok(r.block.map(Enrichment::recalled).into_iter().collect())
    }
}

impl OptionalModule for MemoryModule {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn on_input(
        &self,
        event: &Envelope,
        _class: InputClass,
    ) -> Result<Vec<Enrichment>, ModuleError> {
        let Payload::InputReceived(input) = &event.payload else {
            return Ok(Vec::new());
        };
        let ctx = self.ctx(event);
        if !self.service.enabled(&ctx.project_id) {
            return Ok(Vec::new());
        }
        if let Some(d) = memory_directive(&input.text) {
            // Explicit, user-authored: stored immediately with trust `user`.
            self.service.remember_directive(&ctx, &d.text, &event.id);
        }
        self.recall(&ctx, &input.text, self.service.config().budget_tokens)
    }

    fn on_context_request(
        &self,
        event: &Envelope,
        request: &ContextRequested,
    ) -> Result<Vec<Enrichment>, ModuleError> {
        let ctx = self.ctx(event);
        if !self.service.enabled(&ctx.project_id) {
            return Ok(Vec::new());
        }
        let Some(q) = request.query.as_deref() else {
            return Ok(Vec::new());
        };
        self.recall(&ctx, q, request.budget_tokens)
    }

    /// Tool-call events reach modules only through `advise` (the Guardian owns their
    /// verdict), so the observation of "this command ran and succeeded" lives here. It
    /// never produces advisories.
    fn advise(&self, event: &Envelope) -> Result<Vec<Advisory>, ModuleError> {
        match &event.payload {
            Payload::ToolCallRequested(t) => {
                if let Some(cmd) = &t.command {
                    let entry = Some((t.tool.clone(), cmd.clone()));
                    self.with_state(&event.session_id, |s| s.last_command = entry);
                }
            }
            Payload::ToolCallCompleted(t) => {
                let last = self.with_state(&event.session_id, |s| s.last_command.take());
                let ctx = self.ctx(event);
                if let (true, true, Some((tool, cmd))) =
                    (self.service.enabled(&ctx.project_id), t.ok, last)
                {
                    if tool == t.tool {
                        if let Some(c) = verification_command(&cmd) {
                            self.service.queue_candidate(
                                &ctx,
                                &format!(
                                    "Observed: `{c}` completed successfully in this project (derived from a tool call, not stated by the user)"
                                ),
                                Some("verified_command"),
                                &event.id,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(Vec::new())
    }

    fn on_observe(&self, event: &Envelope) -> Result<(), ModuleError> {
        let ctx = self.ctx(event);
        let enabled = self.service.enabled(&ctx.project_id);
        match &event.payload {
            Payload::MemoryCandidate(c) => {
                if enabled {
                    self.service
                        .queue_candidate(&ctx, &c.text, c.category.as_deref(), &event.id);
                }
            }
            Payload::FilesChanged(f) => {
                self.with_state(&ctx.session_id, |s| {
                    for p in &f.paths {
                        if s.files.len() >= MAX_FILES_TRACKED {
                            break;
                        }
                        if !kepler_policy::sensitive::is_sensitive_path(p) && !s.files.contains(p) {
                            s.files.push(p.clone());
                        }
                    }
                });
            }
            Payload::AgentSettled(s) => {
                let files = self.with_state(&ctx.session_id, |st| std::mem::take(&mut st.files));
                if s.outcome == Outcome::Ok && enabled {
                    if let (Some(b), false) = (&ctx.branch, files.is_empty()) {
                        self.service.queue_scoped(
                            &ctx,
                            &format!(
                                "Observed: the last completed task on branch `{b}` changed: {}",
                                files.join(", ")
                            ),
                            Some("branch_work"),
                            &event.id,
                            true,
                        );
                    }
                    self.service.consolidate(&ctx, &event.id);
                } else {
                    self.service.discard_pending(&ctx.session_id);
                }
            }
            Payload::SessionClosed(_) => {
                self.service.discard_pending(&ctx.session_id);
                self.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&ctx.session_id);
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::MemoryConfig;
    use kepler_policy::{Decision, PolicyEngine};
    use kepler_storage::{Writer, WriterConfig};
    use std::time::Duration;

    fn ev(kind: &str, session: &str, payload: serde_json::Value) -> Envelope {
        let json = serde_json::json!({
            "version": 1, "id": format!("e-{kind}"), "type": kind,
            "timestamp": "2026-01-02T03:04:05Z", "harness": "pi",
            "project_id": "proj", "session_id": session, "correlation_id": "c",
            "payload": payload
        });
        Envelope::parse(json.to_string().as_bytes()).unwrap()
    }

    fn engine() -> (PolicyEngine, Arc<Writer>) {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "kepler-memmod-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("k.db");
        let w = Arc::new(Writer::start(&path, WriterConfig::default()).unwrap());
        let svc = Arc::new(MemoryService::new(w.clone(), path, MemoryConfig::default()));
        let mut e = PolicyEngine::new(Duration::from_millis(500));
        e.register(Box::new(MemoryModule::new(svc)));
        (e, w)
    }

    #[test]
    fn end_to_end_remember_then_recall_across_sessions() {
        let (e, w) = engine();
        let v = e.evaluate(&ev(
            "input.received",
            "s1",
            serde_json::json!({"text": "Remember that the release branch is called stable-2 and we squash merge"}),
        ));
        assert!(v.degraded.is_empty(), "{:?}", v.degraded);
        assert!(w.flush(Duration::from_secs(2)));
        let v = e.evaluate(&ev(
            "input.received",
            "s2",
            serde_json::json!({"text": "which branch do we merge the release into and how"}),
        ));
        match v.decision {
            Decision::Enrich { derived } => {
                assert_eq!(derived[0].source, "memory");
                assert_eq!(derived[0].provenance, "recalled");
                assert!(derived[0].text.contains("stable-2"));
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn candidates_become_memory_only_after_ok_settlement() {
        let (e, w) = engine();
        let cand = |s| {
            ev(
                "memory.candidate",
                s,
                serde_json::json!({"text": "integration tests need docker compose up first", "category": "setup"}),
            )
        };
        e.evaluate(&cand("s1"));
        e.evaluate(&ev(
            "agent.settled",
            "s1",
            serde_json::json!({"outcome": "error", "duration_ms": 5}),
        ));
        assert!(w.flush(Duration::from_secs(2)));
        let ask = ev(
            "input.received",
            "s3",
            serde_json::json!({"text": "how do I run the integration tests with docker"}),
        );
        assert!(e.evaluate(&ask).decision.is_passthrough());

        e.evaluate(&cand("s1"));
        e.evaluate(&ev(
            "agent.settled",
            "s1",
            serde_json::json!({"outcome": "ok", "duration_ms": 5}),
        ));
        assert!(w.flush(Duration::from_secs(2)));
        assert!(matches!(e.evaluate(&ask).decision, Decision::Enrich { .. }));
    }

    #[test]
    fn verification_commands_are_recognised_conservatively() {
        for ok in [
            "cargo test --workspace",
            "npm run build",
            "pnpm test",
            "python -m pytest -q",
            "go test ./...",
            "dotnet build",
        ] {
            assert!(verification_command(ok).is_some(), "{ok}");
        }
        for no in [
            "cargo test && rm -rf x",
            "cargo test | tee x",
            "echo hi",
            "npm install",
            "cargo run --release",
            "cargo test $(cat .env)",
            "",
        ] {
            assert!(verification_command(no).is_none(), "{no}");
        }
    }

    fn tool(session: &str, kind: &str, payload: serde_json::Value) -> Envelope {
        ev(kind, session, payload)
    }

    #[test]
    fn verified_command_is_captured_from_real_tool_events_after_ok_settlement() {
        let (e, w) = engine();
        e.evaluate(&tool(
            "s1",
            "tool.call.requested",
            serde_json::json!({"tool": "bash", "command": "cargo clippy --workspace"}),
        ));
        e.evaluate(&tool(
            "s1",
            "tool.call.completed",
            serde_json::json!({"tool": "bash", "ok": true, "output_bytes": 3, "duration_ms": 9}),
        ));
        // Failed commands are never recorded.
        e.evaluate(&tool(
            "s1",
            "tool.call.requested",
            serde_json::json!({"tool": "bash", "command": "cargo test --features broken"}),
        ));
        e.evaluate(&tool(
            "s1",
            "tool.call.completed",
            serde_json::json!({"tool": "bash", "ok": false, "output_bytes": 3, "duration_ms": 9}),
        ));
        e.evaluate(&ev(
            "agent.settled",
            "s1",
            serde_json::json!({"outcome": "ok", "duration_ms": 5}),
        ));
        assert!(w.flush(Duration::from_secs(2)));
        let ask = ev(
            "input.received",
            "s2",
            serde_json::json!({"text": "which clippy command passes in this workspace"}),
        );
        match e.evaluate(&ask).decision {
            Decision::Enrich { derived } => {
                assert!(derived[0].text.contains("cargo clippy --workspace"));
                assert!(!derived[0].text.contains("broken"));
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn branch_scope_is_selected_from_the_branch_in_events() {
        let (e, w) = engine();
        let inp = |s: &str, branch: Option<&str>, text: &str| {
            let mut p = serde_json::json!({ "text": text });
            if let Some(b) = branch {
                p["branch"] = b.into();
            }
            ev("input.received", s, p)
        };
        e.evaluate(&inp("s1", Some("feat/zeta"), "work on the zeta importer"));
        e.evaluate(&tool(
            "s1",
            "files.changed",
            serde_json::json!({"paths": ["src/zeta_importer.rs", ".env"]}),
        ));
        e.evaluate(&ev(
            "agent.settled",
            "s1",
            serde_json::json!({"outcome": "ok", "duration_ms": 5}),
        ));
        assert!(w.flush(Duration::from_secs(2)));
        let q = "what changed in the zeta importer last task";
        let on_branch = e.evaluate(&inp("s2", Some("feat/zeta"), q));
        match on_branch.decision {
            Decision::Enrich { derived } => {
                assert!(derived[0].text.contains("src/zeta_importer.rs"));
                assert!(!derived[0].text.contains(".env"));
            }
            d => panic!("branch memory not recalled: {d:?}"),
        }
        // Another branch, or an adapter that sends no branch, must not see it.
        assert!(e
            .evaluate(&inp("s3", Some("main"), q))
            .decision
            .is_passthrough());
        assert!(e.evaluate(&inp("s4", None, q)).decision.is_passthrough());
    }

    #[test]
    fn context_request_uses_query_and_budget() {
        let (e, w) = engine();
        e.evaluate(&ev(
            "input.received",
            "s1",
            serde_json::json!({"text": "remember that parser fuzzing runs nightly on the corpus bucket"}),
        ));
        assert!(w.flush(Duration::from_secs(2)));
        let v = e.evaluate(&ev(
            "context.requested",
            "s2",
            serde_json::json!({"budget_tokens": 200, "query": "parser fuzzing nightly"}),
        ));
        assert!(matches!(v.decision, Decision::Enrich { .. }));
        let none = e.evaluate(&ev(
            "context.requested",
            "s2",
            serde_json::json!({"budget_tokens": 200}),
        ));
        assert!(none.decision.is_passthrough());
    }
}
