use crate::config::FocusConfig;
use crate::state::SessionState;
use kepler_events::{Envelope, Payload};
use kepler_policy::{classify_input, Advisory, InputClass, ModuleError, OptionalModule};
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
struct Sessions {
    map: HashMap<String, SessionState>,
    tick: u64,
}

/// Focus as a pipeline module. It adds no derived context and never decides allow/deny:
/// it only contributes [`Advisory`] notes through [`OptionalModule::advise`].
pub struct FocusModule {
    cfg: FocusConfig,
    sessions: Mutex<Sessions>,
}

impl FocusModule {
    pub fn new(cfg: FocusConfig) -> Self {
        Self {
            cfg,
            sessions: Mutex::new(Sessions::default()),
        }
    }

    /// Sessions currently tracked (bounded by `max_sessions`).
    pub fn tracked_sessions(&self) -> usize {
        self.lock().map.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Sessions> {
        // A poisoned lock only means an earlier panic was contained; the data is still usable.
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for FocusModule {
    fn default() -> Self {
        Self::new(FocusConfig::default())
    }
}

impl OptionalModule for FocusModule {
    fn name(&self) -> &'static str {
        "focus"
    }

    fn on_input(
        &self,
        _event: &Envelope,
        _class: InputClass,
    ) -> Result<Vec<kepler_policy::Enrichment>, ModuleError> {
        Ok(Vec::new())
    }

    fn advise(&self, event: &Envelope) -> Result<Vec<Advisory>, ModuleError> {
        let mut s = self.lock();
        s.tick += 1;
        let tick = s.tick;
        let key = event.session_id.clone();
        if matches!(event.payload, Payload::SessionClosed(_)) {
            s.map.remove(&key);
            return Ok(Vec::new());
        }
        if !s.map.contains_key(&key) && s.map.len() >= self.cfg.max_sessions {
            let oldest = s
                .map
                .iter()
                .min_by_key(|(_, v)| v.last_used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                s.map.remove(&k);
            }
        }
        let st = s.map.entry(key).or_default();
        st.last_used = tick;
        if let Payload::InputReceived(input) = &event.payload {
            if !matches!(classify_input(&input.text), InputClass::Trivial(_)) {
                st.begin_task(&input.text);
            }
            return Ok(Vec::new());
        }
        Ok(st.observe(&self.cfg, event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kepler_policy::{AdvisoryLevel, PolicyEngine};
    use std::sync::Arc;

    fn ev(session: &str, kind: &str, payload: serde_json::Value) -> Envelope {
        let json = serde_json::json!({
            "version": 1, "id": "e1", "type": kind, "timestamp": "2026-01-02T03:04:05Z",
            "harness": "pi", "project_id": "p", "session_id": session, "correlation_id": "c",
            "payload": payload,
        });
        Envelope::parse(json.to_string().as_bytes()).unwrap()
    }

    fn input(s: &str, text: &str) -> Envelope {
        ev(s, "input.received", serde_json::json!({ "text": text }))
    }
    fn call(s: &str, tool: &str, cmd: &str) -> Envelope {
        ev(
            s,
            "tool.call.requested",
            serde_json::json!({"tool": tool, "command": cmd}),
        )
    }
    fn done(s: &str, tool: &str, ok: bool) -> Envelope {
        ev(
            s,
            "tool.call.completed",
            serde_json::json!({"tool": tool, "ok": ok, "output_bytes": 1, "duration_ms": 1}),
        )
    }
    fn files(s: &str, paths: &[&str]) -> Envelope {
        ev(s, "files.changed", serde_json::json!({ "paths": paths }))
    }
    fn settled(s: &str) -> Envelope {
        ev(
            s,
            "agent.settled",
            serde_json::json!({"outcome": "ok", "duration_ms": 5}),
        )
    }

    fn engine(cfg: FocusConfig) -> PolicyEngine {
        let mut e = PolicyEngine::default();
        e.register(Box::new(FocusModule::new(cfg)));
        e
    }

    fn codes(e: &PolicyEngine, ev: &Envelope) -> Vec<String> {
        e.evaluate(ev)
            .advisories
            .into_iter()
            .map(|a| a.code)
            .collect()
    }

    const LONG: &str = "please refactor the parser module in src/parser.rs carefully";

    #[test]
    fn repeated_identical_calls_warn_but_legitimate_retries_after_a_change_do_not() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        let mut got = Vec::new();
        for _ in 0..4 {
            got = codes(&e, &call("s", "bash", "cargo test"));
        }
        assert_eq!(got, vec!["loop.repeated_call"]);

        // A file change between attempts makes the retry legitimate.
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        for _ in 0..6 {
            assert!(codes(&e, &call("s", "bash", "cargo test")).is_empty());
            e.evaluate(&files("s", &["src/parser.rs"]));
        }
    }

    #[test]
    fn same_failure_without_change_warns_once_then_cools_down() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        let mut seen = Vec::new();
        for _ in 0..4 {
            e.evaluate(&call("s", "bash", "npm run build"));
            seen.extend(codes(&e, &done("s", "bash", false)));
        }
        assert_eq!(seen, vec!["loop.repeated_failure"]);
    }

    #[test]
    fn oscillating_edits_are_flagged() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        let mut all = Vec::new();
        for p in ["a.rs", "b.rs", "a.rs", "b.rs"] {
            all.extend(codes(&e, &files("s", &[p])));
        }
        assert!(
            all.contains(&"loop.oscillating_changes".to_string()),
            "{all:?}"
        );
    }

    #[test]
    fn dependency_changes_are_flagged_only_when_not_requested() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        assert_eq!(
            codes(&e, &files("s", &["Cargo.toml"])),
            vec!["minimal.dependency_change"]
        );
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", "please add the serde dependency to the crate"));
        assert!(codes(&e, &files("s", &["Cargo.toml"])).is_empty());
    }

    #[test]
    fn unrelated_files_are_flagged_after_a_threshold_not_for_the_named_file() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        assert!(codes(&e, &files("s", &["src/parser.rs"])).is_empty());
        assert!(codes(&e, &files("s", &["docs/a.md", "web/b.ts"])).is_empty());
        assert_eq!(
            codes(&e, &files("s", &["ci/c.yml"])),
            vec!["scope.unrelated_changes"]
        );
    }

    #[test]
    fn budget_warns_while_progressing_and_recommends_stop_when_stalled() {
        let cfg = FocusConfig {
            soft_tool_calls: 3,
            stall_calls: 4,
            cooldown_events: 1,
            ..FocusConfig::default()
        };
        let e = engine(cfg);
        e.evaluate(&input("s", LONG));
        let mut levels = Vec::new();
        for i in 0..8 {
            e.evaluate(&files("s", &[&format!("src/f{i}.rs")]));
            let v = e.evaluate(&call("s", "read", &format!("cat {i}")));
            levels.extend(v.advisories.into_iter().map(|a| a.code));
        }
        assert!(levels.iter().all(|c| c == "budget.exceeded"), "{levels:?}");
        assert!(!levels.is_empty());
        let mut stalled = Vec::new();
        for i in 0..10 {
            let v = e.evaluate(&call("s", "read", &format!("ls {i}")));
            stalled.extend(v.advisories);
        }
        let last = stalled.iter().find(|a| a.code == "budget.stalled").unwrap();
        assert_eq!(last.level, AdvisoryLevel::RecommendStop);
    }

    #[test]
    fn completion_gate_reports_unknown_verification_and_observed_verification() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        e.evaluate(&files("s", &["src/parser.rs"]));
        assert_eq!(
            codes(&e, &settled("s")),
            vec!["evidence.unverified_completion"]
        );

        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        e.evaluate(&files("s", &["src/parser.rs"]));
        e.evaluate(&call("s", "bash", "cargo test"));
        e.evaluate(&done("s", "bash", true));
        assert_eq!(codes(&e, &settled("s")), vec!["completion.verified"]);
        // No change at all: nothing to verify, nothing to say.
        let e = engine(FocusConfig::default());
        assert!(codes(&e, &settled("s2")).is_empty());
    }

    #[test]
    fn recommends_stopping_after_verification_when_nothing_changes() {
        let cfg = FocusConfig {
            post_verify_calls: 3,
            ..FocusConfig::default()
        };
        let e = engine(cfg);
        e.evaluate(&input("s", LONG));
        e.evaluate(&files("s", &["src/parser.rs"]));
        e.evaluate(&call("s", "bash", "cargo test"));
        e.evaluate(&done("s", "bash", true));
        let mut all = Vec::new();
        for i in 0..4 {
            all.extend(codes(&e, &call("s", "read", &format!("f{i}"))));
        }
        assert_eq!(all, vec!["completion.recommend_stop"]);
    }

    #[test]
    fn a_new_user_input_restarts_the_task() {
        let e = engine(FocusConfig::default());
        e.evaluate(&input("s", LONG));
        for _ in 0..3 {
            e.evaluate(&call("s", "bash", "ls"));
        }
        e.evaluate(&input(
            "s",
            "now please update the changelog in CHANGELOG.md",
        ));
        assert!(codes(&e, &call("s", "bash", "ls")).is_empty());
    }

    #[test]
    fn sessions_are_bounded_and_released_on_close() {
        let m = Arc::new(FocusModule::new(FocusConfig {
            max_sessions: 3,
            ..FocusConfig::default()
        }));
        for i in 0..10 {
            m.advise(&call(&format!("s{i}"), "bash", "ls")).unwrap();
        }
        assert_eq!(m.tracked_sessions(), 3);
        m.advise(&ev("s9", "session.closed", serde_json::json!({})))
            .unwrap();
        assert_eq!(m.tracked_sessions(), 2);
    }

    #[test]
    fn advisories_never_change_the_decision_and_carry_no_prompt_text() {
        let e = engine(FocusConfig::default());
        let secret = "please refactor src/a.rs and keep the password hunter2 out of logs";
        let v = e.evaluate(&input("s", secret));
        assert!(v.decision.is_passthrough());
        for _ in 0..5 {
            let v = e.evaluate(&call("s", "bash", "ls"));
            assert!(v.decision.is_passthrough());
            for a in v.advisories {
                assert!(!a.message.contains("hunter2"));
            }
        }
    }
}
