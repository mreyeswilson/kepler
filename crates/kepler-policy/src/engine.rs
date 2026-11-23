use crate::classify::{classify_input, InputClass};
use crate::decision::{Decision, Derived, Verdict};
use crate::preflight::preflight_tool_call;
use kepler_events::{Envelope, Payload};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

pub const MAX_MODULES: usize = 16;
const MAX_DERIVED_BYTES: usize = 8 * 1024;
const MAX_DEGRADED_NOTES: usize = 16;

/// Derived context proposed by an optional module. It never replaces the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrichment {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleError(pub String);

/// Optional, non-security module (Forge/Memory/... in later deliveries).
/// Any error, panic, slow answer or oversized output degrades to passthrough.
pub trait OptionalModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn on_input(
        &self,
        event: &Envelope,
        class: InputClass,
    ) -> Result<Option<Enrichment>, ModuleError>;
}

pub struct PolicyEngine {
    modules: Vec<Box<dyn OptionalModule>>,
    budget: Duration,
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new(Duration::from_millis(50))
    }
}

impl PolicyEngine {
    /// `budget` is the total time optional modules may use per event.
    pub fn new(budget: Duration) -> Self {
        Self {
            modules: Vec::new(),
            budget,
        }
    }

    /// Register an optional module. Returns `false` (and drops it) past [`MAX_MODULES`].
    pub fn register(&mut self, m: Box<dyn OptionalModule>) -> bool {
        if self.modules.len() >= MAX_MODULES {
            return false;
        }
        self.modules.push(m);
        true
    }

    /// Evaluate one validated event. Infallible by construction.
    pub fn evaluate(&self, event: &Envelope) -> Verdict {
        match &event.payload {
            Payload::ToolCallRequested(call) => {
                // Security boundary: no verdict (panic) => deny, never passthrough.
                let d = catch_unwind(AssertUnwindSafe(|| preflight_tool_call(call)))
                    .unwrap_or_else(|_| Decision::Deny {
                        reason: "preflight_failure".into(),
                    });
                Verdict::new(d)
            }
            Payload::InputReceived(input) => self.evaluate_input(event, &input.text),
            _ => Verdict::new(Decision::passthrough("observe_only")),
        }
    }

    fn evaluate_input(&self, event: &Envelope, text: &str) -> Verdict {
        let class = classify_input(text);
        if let InputClass::Trivial(why) = class {
            return Verdict::new(Decision::passthrough(format!("trivial_input:{why}")));
        }
        let start = Instant::now();
        let mut derived = Vec::new();
        let mut degraded = Vec::new();
        for m in &self.modules {
            if start.elapsed() >= self.budget {
                note(
                    &mut degraded,
                    format!("{}: skipped, budget exhausted", m.name()),
                );
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| m.on_input(event, class))) {
                Err(_) => note(&mut degraded, format!("{}: panicked", m.name())),
                Ok(Err(e)) => note(
                    &mut degraded,
                    format!("{}: error: {}", m.name(), clip(&e.0)),
                ),
                Ok(Ok(None)) => {}
                Ok(Ok(Some(_))) if start.elapsed() > self.budget => {
                    note(
                        &mut degraded,
                        format!("{}: over budget, result discarded", m.name()),
                    );
                }
                Ok(Ok(Some(e))) if e.text.len() > MAX_DERIVED_BYTES || e.text.contains('\0') => {
                    note(
                        &mut degraded,
                        format!("{}: invalid output discarded", m.name()),
                    );
                }
                Ok(Ok(Some(e))) => derived.push(Derived::inferred(m.name(), e.text)),
            }
        }
        let decision = if derived.is_empty() {
            Decision::passthrough("no_enrichment")
        } else {
            Decision::Enrich { derived }
        };
        Verdict { decision, degraded }
    }
}

fn note(v: &mut Vec<String>, s: String) {
    if v.len() < MAX_DEGRADED_NOTES {
        v.push(s);
    }
}

fn clip(s: &str) -> String {
    s.chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, payload: &str) -> Envelope {
        let json = format!(
            r#"{{"version":1,"id":"e1","type":"{kind}","timestamp":"2026-01-02T03:04:05Z","harness":"pi","project_id":"p","session_id":"s","correlation_id":"c","payload":{payload}}}"#
        );
        Envelope::parse(json.as_bytes()).unwrap()
    }

    fn input(text: &str) -> Envelope {
        ev(
            "input.received",
            &serde_json::json!({ "text": text }).to_string(),
        )
    }

    struct Fixed(Result<Option<Enrichment>, ModuleError>);
    impl OptionalModule for Fixed {
        fn name(&self) -> &'static str {
            "fixed"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Result<Option<Enrichment>, ModuleError> {
            self.0.clone()
        }
    }
    struct Panics;
    impl OptionalModule for Panics {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Result<Option<Enrichment>, ModuleError> {
            panic!("boom")
        }
    }
    struct Slow;
    impl OptionalModule for Slow {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Result<Option<Enrichment>, ModuleError> {
            std::thread::sleep(Duration::from_millis(30));
            Ok(Some(Enrichment {
                text: "late".into(),
            }))
        }
    }
    struct Counting(std::sync::atomic::AtomicUsize);
    impl OptionalModule for std::sync::Arc<Counting> {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Result<Option<Enrichment>, ModuleError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
    }

    #[test]
    fn default_is_passthrough() {
        let e = PolicyEngine::default();
        let v = e.evaluate(&input("Fix the failing test in parser.rs"));
        assert_eq!(v.decision, Decision::passthrough("no_enrichment"));
        assert!(v.degraded.is_empty());
    }

    #[test]
    fn trivial_input_passthrough_without_running_modules() {
        let counter = std::sync::Arc::new(Counting(Default::default()));
        let mut e = PolicyEngine::default();
        e.register(Box::new(counter.clone()));
        for t in ["ok", "thanks!", "/help", ""] {
            assert!(e.evaluate(&input(t)).decision.is_passthrough());
        }
        assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 0);
        e.evaluate(&input("please refactor the parser module"));
        assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn non_input_events_are_observed_only() {
        let e = PolicyEngine::default();
        let v = e.evaluate(&ev("session.closed", "{}"));
        assert_eq!(v.decision, Decision::passthrough("observe_only"));
    }

    #[test]
    fn module_failures_fall_back_to_passthrough() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed(Err(ModuleError("db down".into())))));
        e.register(Box::new(Panics));
        e.register(Box::new(Fixed(Ok(Some(Enrichment {
            text: "x".repeat(9000),
        })))));
        e.register(Box::new(Fixed(Ok(Some(Enrichment {
            text: "nul\0".into(),
        })))));
        let v = e.evaluate(&input("refactor the parser please"));
        assert_eq!(v.decision, Decision::passthrough("no_enrichment"));
        assert_eq!(v.degraded.len(), 4, "{:?}", v.degraded);
    }

    #[test]
    fn over_budget_result_is_discarded_and_later_modules_skipped() {
        let mut e = PolicyEngine::new(Duration::from_millis(5));
        e.register(Box::new(Slow));
        e.register(Box::new(Fixed(Ok(Some(Enrichment {
            text: "never".into(),
        })))));
        let v = e.evaluate(&input("refactor the parser please"));
        assert!(v.decision.is_passthrough());
        assert!(v.degraded.iter().any(|d| d.contains("over budget")));
        assert!(v.degraded.iter().any(|d| d.contains("budget exhausted")));
    }

    #[test]
    fn successful_enrichment_is_separate_and_labeled_inferred() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed(Ok(Some(Enrichment {
            text: "uses cargo".into(),
        })))));
        let original = input("refactor the parser please");
        let before = original.clone();
        let v = e.evaluate(&original);
        assert_eq!(original, before, "original must be untouched");
        match v.decision {
            Decision::Enrich { derived } => {
                assert_eq!(derived.len(), 1);
                assert_eq!(derived[0].provenance, "inferred");
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn module_cap_is_enforced() {
        let mut e = PolicyEngine::default();
        for _ in 0..MAX_MODULES {
            assert!(e.register(Box::new(Fixed(Ok(None)))));
        }
        assert!(!e.register(Box::new(Fixed(Ok(None)))));
    }

    #[test]
    fn tool_preflight_runs_even_with_failing_modules() {
        let mut e = PolicyEngine::default();
        e.register(Box::new(Panics));
        let v = e.evaluate(&ev(
            "tool.call.requested",
            r#"{"tool":"bash","command":"cat .env"}"#,
        ));
        assert!(matches!(v.decision, Decision::Deny { .. }));
        let v = e.evaluate(&ev(
            "tool.call.requested",
            r#"{"tool":"bash","command":"rm -rf /"}"#,
        ));
        assert!(matches!(v.decision, Decision::RequireApproval { .. }));
        let v = e.evaluate(&ev(
            "tool.call.requested",
            r#"{"tool":"bash","command":"cargo build"}"#,
        ));
        assert!(v.decision.is_passthrough());
    }

    #[test]
    fn evaluation_is_deterministic() {
        let e = PolicyEngine::default();
        let ev = input("Fix the failing test in parser.rs");
        assert_eq!(e.evaluate(&ev), e.evaluate(&ev));
    }
}
