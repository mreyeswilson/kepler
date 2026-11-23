use crate::classify::{classify_input, InputClass};
use crate::decision::{Decision, Derived, Verdict};
use crate::guardian::Guardian;
use crate::tokens::{estimate_tokens, ModelFamily};
use kepler_events::{ContextRequested, Envelope, Payload};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

pub const MAX_MODULES: usize = 16;
pub const MAX_STAGES: usize = 4;
const MAX_DERIVED_BYTES: usize = 8 * 1024;
/// Hard cap for all derived text of one verdict (applies even when a stage fails).
pub const MAX_TOTAL_DERIVED_BYTES: usize = 16 * 1024;
const MAX_DEGRADED_NOTES: usize = 16;

/// Where a piece of derived text came from. Neither is ever a user instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Produced by deterministic rules from the user's input (Forge).
    Inferred,
    /// Read back from local memory; carries a trust level in its text (Memory).
    Recalled,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inferred => "inferred",
            Self::Recalled => "recalled",
        }
    }
}

/// Derived context proposed by an optional module. It never replaces the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrichment {
    pub text: String,
    pub provenance: Provenance,
}

impl Enrichment {
    pub fn inferred(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provenance: Provenance::Inferred,
        }
    }
    pub fn recalled(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provenance: Provenance::Recalled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleError(pub String);

/// Optional, non-security module (Forge, Memory). Modules run in registration order,
/// which is the documented pipeline order. Any error, panic, slow answer or oversized
/// output degrades to passthrough.
pub trait OptionalModule: Send + Sync {
    fn name(&self) -> &'static str;
    fn on_input(&self, event: &Envelope, class: InputClass)
        -> Result<Vec<Enrichment>, ModuleError>;
    /// A harness explicitly asked for context (`context.requested`).
    fn on_context_request(
        &self,
        _event: &Envelope,
        _request: &ContextRequested,
    ) -> Result<Vec<Enrichment>, ModuleError> {
        Ok(Vec::new())
    }
    /// Any event that is neither an input, a context request nor a tool preflight
    /// (settlement, memory candidates, session close, ...). Observation only.
    fn on_observe(&self, _event: &Envelope) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// Runs after all modules (Guardian): may drop, dedupe or reorder derived items,
/// never add instructions. A failing stage leaves the list untouched.
pub trait OutputStage: Send + Sync {
    fn name(&self) -> &'static str;
    fn refine(&self, event: &Envelope, derived: Vec<Derived>) -> Result<Vec<Derived>, ModuleError>;
}

pub struct PolicyEngine {
    modules: Vec<Box<dyn OptionalModule>>,
    stages: Vec<Box<dyn OutputStage>>,
    budget: Duration,
    guardian: Guardian,
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
            stages: Vec::new(),
            budget,
            guardian: Guardian::default(),
        }
    }

    /// Replace the Guardian (e.g. with user rules). Rules only add restrictions.
    pub fn set_guardian(&mut self, g: Guardian) {
        self.guardian = g;
    }

    pub fn guardian(&self) -> &Guardian {
        &self.guardian
    }

    /// Register an optional module. Returns `false` (and drops it) past [`MAX_MODULES`].
    pub fn register(&mut self, m: Box<dyn OptionalModule>) -> bool {
        if self.modules.len() >= MAX_MODULES {
            return false;
        }
        self.modules.push(m);
        true
    }

    /// Register an output stage. Returns `false` past [`MAX_STAGES`].
    pub fn register_stage(&mut self, s: Box<dyn OutputStage>) -> bool {
        if self.stages.len() >= MAX_STAGES {
            return false;
        }
        self.stages.push(s);
        true
    }

    /// Names of registered modules then stages, in execution order.
    pub fn pipeline(&self) -> Vec<&'static str> {
        self.modules
            .iter()
            .map(|m| m.name())
            .chain(self.stages.iter().map(|s| s.name()))
            .collect()
    }

    /// Evaluate one validated event. Infallible by construction.
    pub fn evaluate(&self, event: &Envelope) -> Verdict {
        match &event.payload {
            Payload::ToolCallRequested(call) => {
                // Security boundary: no verdict (panic) => deny, never passthrough.
                let d = catch_unwind(AssertUnwindSafe(|| self.guardian.preflight(call)))
                    .unwrap_or_else(|_| Decision::Deny {
                        reason: "preflight_failure".into(),
                    });
                Verdict::new(d)
            }
            Payload::ToolCallCompleted(done) => {
                // Security check, fail closed: if the scan cannot run, withhold the output.
                let scan = catch_unwind(AssertUnwindSafe(|| self.guardian.scan_result(done)))
                    .unwrap_or_else(|_| Decision::Redact {
                        reason: "scan_failure".into(),
                        kinds: Vec::new(),
                    });
                match scan {
                    Decision::Redact { .. } => Verdict::new(scan),
                    _ => self.observe(event),
                }
            }
            Payload::InputReceived(input) => {
                let class = classify_input(&input.text);
                if let InputClass::Trivial(why) = class {
                    return Verdict::new(Decision::passthrough(format!("trivial_input:{why}")));
                }
                self.enrich(event, |m| m.on_input(event, class))
            }
            Payload::ContextRequested(req) => {
                self.enrich(event, |m| m.on_context_request(event, req))
            }
            _ => self.observe(event),
        }
    }

    fn observe(&self, event: &Envelope) -> Verdict {
        let start = Instant::now();
        let mut degraded = Vec::new();
        for m in &self.modules {
            if start.elapsed() >= self.budget {
                note(
                    &mut degraded,
                    format!("{}: skipped, budget exhausted", m.name()),
                );
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| m.on_observe(event))) {
                Err(_) => note(&mut degraded, format!("{}: panicked", m.name())),
                Ok(Err(e)) => note(
                    &mut degraded,
                    format!("{}: error: {}", m.name(), clip(&e.0)),
                ),
                Ok(Ok(())) => {}
            }
        }
        Verdict {
            decision: Decision::passthrough("observe_only"),
            degraded,
            estimated_tokens: None,
        }
    }

    fn enrich<F>(&self, event: &Envelope, run: F) -> Verdict
    where
        F: Fn(&dyn OptionalModule) -> Result<Vec<Enrichment>, ModuleError>,
    {
        let start = Instant::now();
        let mut derived: Vec<Derived> = Vec::new();
        let mut degraded = Vec::new();
        for m in &self.modules {
            if start.elapsed() >= self.budget {
                note(
                    &mut degraded,
                    format!("{}: skipped, budget exhausted", m.name()),
                );
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| run(m.as_ref()))) {
                Err(_) => note(&mut degraded, format!("{}: panicked", m.name())),
                Ok(Err(e)) => note(
                    &mut degraded,
                    format!("{}: error: {}", m.name(), clip(&e.0)),
                ),
                Ok(Ok(items)) if items.is_empty() => {}
                Ok(Ok(_)) if start.elapsed() > self.budget => {
                    note(
                        &mut degraded,
                        format!("{}: over budget, result discarded", m.name()),
                    );
                }
                Ok(Ok(items)) => {
                    for e in items {
                        if e.text.trim().is_empty() {
                            continue;
                        }
                        if e.text.len() > MAX_DERIVED_BYTES || e.text.contains('\0') {
                            note(
                                &mut degraded,
                                format!("{}: invalid output discarded", m.name()),
                            );
                        } else {
                            derived.push(Derived::new(m.name(), e.text, e.provenance.as_str()));
                        }
                    }
                }
            }
        }
        for s in &self.stages {
            let before = derived.clone();
            match catch_unwind(AssertUnwindSafe(|| s.refine(event, before))) {
                Err(_) => note(&mut degraded, format!("{}: panicked", s.name())),
                Ok(Err(e)) => note(
                    &mut degraded,
                    format!("{}: error: {}", s.name(), clip(&e.0)),
                ),
                Ok(Ok(refined)) => derived = refined,
            }
        }
        cap_total(&mut derived, &mut degraded);
        if derived.is_empty() {
            return Verdict {
                decision: Decision::passthrough("no_enrichment"),
                degraded,
                estimated_tokens: None,
            };
        }
        let tokens: u32 = derived
            .iter()
            .map(|d| estimate_tokens(&d.text, ModelFamily::Generic).tokens)
            .sum();
        Verdict {
            decision: Decision::Enrich { derived },
            degraded,
            estimated_tokens: Some(tokens),
        }
    }
}

fn cap_total(derived: &mut Vec<Derived>, degraded: &mut Vec<String>) {
    let mut total = 0usize;
    let before = derived.len();
    derived.retain(|d| {
        total += d.text.len();
        total <= MAX_TOTAL_DERIVED_BYTES
    });
    if derived.len() < before {
        note(degraded, "derived output truncated at hard cap".into());
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
    use std::sync::{Arc, Mutex};

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

    type Out = Result<Vec<Enrichment>, ModuleError>;

    struct Fixed(&'static str, Out);
    impl OptionalModule for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Out {
            self.1.clone()
        }
        fn on_context_request(&self, _: &Envelope, _: &ContextRequested) -> Out {
            self.1.clone()
        }
    }
    fn one(text: &str) -> Out {
        Ok(vec![Enrichment::inferred(text)])
    }
    struct Panics;
    impl OptionalModule for Panics {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Out {
            panic!("boom")
        }
        fn on_observe(&self, _: &Envelope) -> Result<(), ModuleError> {
            panic!("boom")
        }
    }
    struct Slow;
    impl OptionalModule for Slow {
        fn name(&self) -> &'static str {
            "slow"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Out {
            std::thread::sleep(Duration::from_millis(30));
            one("late")
        }
    }
    struct Counting(std::sync::atomic::AtomicUsize);
    impl OptionalModule for Arc<Counting> {
        fn name(&self) -> &'static str {
            "counting"
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Out {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![])
        }
    }
    /// Records the order in which modules are called.
    struct Trace(&'static str, Arc<Mutex<Vec<&'static str>>>);
    impl OptionalModule for Trace {
        fn name(&self) -> &'static str {
            self.0
        }
        fn on_input(&self, _: &Envelope, _: InputClass) -> Out {
            self.1.lock().unwrap().push(self.0);
            one(self.0)
        }
        fn on_observe(&self, _: &Envelope) -> Result<(), ModuleError> {
            self.1.lock().unwrap().push(self.0);
            Ok(())
        }
    }

    struct KeepFirst;
    impl OutputStage for KeepFirst {
        fn name(&self) -> &'static str {
            "keep_first"
        }
        fn refine(&self, _: &Envelope, mut d: Vec<Derived>) -> Result<Vec<Derived>, ModuleError> {
            d.truncate(1);
            Ok(d)
        }
    }
    struct BrokenStage;
    impl OutputStage for BrokenStage {
        fn name(&self) -> &'static str {
            "broken"
        }
        fn refine(&self, _: &Envelope, _: Vec<Derived>) -> Result<Vec<Derived>, ModuleError> {
            Err(ModuleError("nope".into()))
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
        let counter = Arc::new(Counting(Default::default()));
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
    fn observe_failures_are_degraded_not_fatal() {
        let mut e = PolicyEngine::default();
        e.register(Box::new(Panics));
        let v = e.evaluate(&ev("session.closed", "{}"));
        assert_eq!(v.decision, Decision::passthrough("observe_only"));
        assert_eq!(v.degraded.len(), 1);
    }

    #[test]
    fn module_failures_fall_back_to_passthrough() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed("a", Err(ModuleError("db down".into())))));
        e.register(Box::new(Panics));
        e.register(Box::new(Fixed("c", one(&"x".repeat(9000)))));
        e.register(Box::new(Fixed("d", one("nul\0"))));
        let v = e.evaluate(&input("refactor the parser please"));
        assert_eq!(v.decision, Decision::passthrough("no_enrichment"));
        assert_eq!(v.degraded.len(), 4, "{:?}", v.degraded);
    }

    #[test]
    fn over_budget_result_is_discarded_and_later_modules_skipped() {
        let mut e = PolicyEngine::new(Duration::from_millis(5));
        e.register(Box::new(Slow));
        e.register(Box::new(Fixed("never", one("never"))));
        let v = e.evaluate(&input("refactor the parser please"));
        assert!(v.decision.is_passthrough());
        assert!(v.degraded.iter().any(|d| d.contains("over budget")));
        assert!(v.degraded.iter().any(|d| d.contains("budget exhausted")));
    }

    #[test]
    fn successful_enrichment_is_separate_and_labeled() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed("f", one("uses cargo"))));
        e.register(Box::new(Fixed(
            "m",
            Ok(vec![Enrichment::recalled("uses pnpm")]),
        )));
        let original = input("refactor the parser please");
        let before = original.clone();
        let v = e.evaluate(&original);
        assert_eq!(original, before, "original must be untouched");
        match v.decision {
            Decision::Enrich { derived } => {
                assert_eq!(derived.len(), 2);
                assert_eq!(derived[0].provenance, "inferred");
                assert_eq!(derived[1].provenance, "recalled");
                assert!(derived.iter().all(|d| d.provenance != "user"));
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn modules_run_in_registration_order_then_stages() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Trace("forge", log.clone())));
        e.register(Box::new(Trace("memory", log.clone())));
        e.register_stage(Box::new(KeepFirst));
        assert_eq!(e.pipeline(), vec!["forge", "memory", "keep_first"]);
        let v = e.evaluate(&input("refactor the parser please"));
        assert_eq!(*log.lock().unwrap(), vec!["forge", "memory"]);
        match v.decision {
            Decision::Enrich { derived } => {
                assert_eq!(derived.len(), 1);
                assert_eq!(derived[0].source, "forge");
            }
            d => panic!("{d:?}"),
        }
        log.lock().unwrap().clear();
        e.evaluate(&ev("agent.settled", r#"{"outcome":"ok","duration_ms":5}"#));
        assert_eq!(*log.lock().unwrap(), vec!["forge", "memory"]);
    }

    #[test]
    fn failing_stage_keeps_module_output() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed("f", one("kept"))));
        e.register_stage(Box::new(BrokenStage));
        let v = e.evaluate(&input("refactor the parser please"));
        assert!(matches!(v.decision, Decision::Enrich { .. }));
        assert!(v.degraded.iter().any(|d| d.starts_with("broken")));
    }

    #[test]
    fn context_requests_reach_modules() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed("f", one("ctx"))));
        let v = e.evaluate(&ev(
            "context.requested",
            r#"{"budget_tokens":100,"query":"parser"}"#,
        ));
        assert!(matches!(v.decision, Decision::Enrich { .. }));
    }

    #[test]
    fn module_cap_is_enforced() {
        let mut e = PolicyEngine::default();
        for _ in 0..MAX_MODULES {
            assert!(e.register(Box::new(Fixed("n", Ok(vec![])))));
        }
        assert!(!e.register(Box::new(Fixed("n", Ok(vec![])))));
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
    fn tool_results_with_secrets_are_redacted_and_never_echoed() {
        let e = PolicyEngine::default();
        let v = e.evaluate(&ev(
            "tool.call.completed",
            r#"{"tool":"bash","ok":true,"output_bytes":40,"duration_ms":3,"output_excerpt":"DB_PASSWORD=hunter2hunter2 and AKIAIOSFODNN7EXAMPLE"}"#,
        ));
        match &v.decision {
            Decision::Redact { reason, kinds } => {
                assert_eq!(reason, "secret_in_tool_result");
                assert!(!kinds.is_empty());
            }
            d => panic!("{d:?}"),
        }
        let wire = serde_json::to_string(&v).unwrap();
        assert!(
            !wire.contains("hunter2") && !wire.contains("AKIAIOSFODNN7EXAMPLE"),
            "{wire}"
        );
        let clean = e.evaluate(&ev(
            "tool.call.completed",
            r#"{"tool":"bash","ok":true,"output_bytes":10,"duration_ms":3,"output_excerpt":"3 tests passed"}"#,
        ));
        assert_eq!(clean.decision, Decision::passthrough("observe_only"));
    }

    #[test]
    fn enrichment_reports_estimated_tokens_only_when_enriched() {
        let mut e = PolicyEngine::new(Duration::from_secs(1));
        e.register(Box::new(Fixed("f", one("some derived text here"))));
        let v = e.evaluate(&input("refactor the parser please"));
        assert!(v.estimated_tokens.is_some_and(|t| t > 0));
        let wire = serde_json::to_value(&v).unwrap();
        assert!(wire["estimated_tokens"].as_u64().unwrap() > 0);
        let none = PolicyEngine::default().evaluate(&input("refactor the parser please"));
        assert!(none.estimated_tokens.is_none());
    }

    #[test]
    fn evaluation_is_deterministic() {
        let e = PolicyEngine::default();
        let ev = input("Fix the failing test in parser.rs");
        assert_eq!(e.evaluate(&ev), e.evaluate(&ev));
    }
}
