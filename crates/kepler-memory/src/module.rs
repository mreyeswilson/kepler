use crate::service::{Ctx, MemoryService};
use kepler_events::{ContextRequested, Envelope, Outcome, Payload};
use kepler_forge::memory_directive;
use kepler_policy::{Enrichment, InputClass, ModuleError, OptionalModule};
use std::sync::Arc;

/// Memory as a pipeline module (after Forge, before Guardian).
pub struct MemoryModule {
    service: Arc<MemoryService>,
}

impl MemoryModule {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self { service }
    }

    fn ctx(event: &Envelope) -> Ctx {
        Ctx {
            project_id: event.project_id.clone(),
            session_id: event.session_id.clone(),
            branch: None,
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
        let ctx = Self::ctx(event);
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
        let ctx = Self::ctx(event);
        if !self.service.enabled(&ctx.project_id) {
            return Ok(Vec::new());
        }
        let Some(q) = request.query.as_deref() else {
            return Ok(Vec::new());
        };
        self.recall(&ctx, q, request.budget_tokens)
    }

    fn on_observe(&self, event: &Envelope) -> Result<(), ModuleError> {
        let ctx = Self::ctx(event);
        match &event.payload {
            Payload::MemoryCandidate(c) => {
                if self.service.enabled(&ctx.project_id) {
                    self.service
                        .queue_candidate(&ctx, &c.text, c.category.as_deref(), &event.id);
                }
            }
            Payload::AgentSettled(s) => {
                if s.outcome == Outcome::Ok && self.service.enabled(&ctx.project_id) {
                    self.service.consolidate(&ctx, &event.id);
                } else {
                    self.service.discard_pending(&ctx.session_id);
                }
            }
            Payload::SessionClosed(_) => {
                self.service.discard_pending(&ctx.session_id);
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
