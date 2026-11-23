#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Focus calibration on labelled session traces (ADR-0023).
//!
//! Each trace is a hand-written, realistic event sequence with the advisories a careful
//! reviewer would want (`want`) and the ones that would be noise (`forbid`). The test prints the
//! detection rate, the false-positive count and the per-event cost, and fails if the default
//! thresholds regress. These are *detector* measurements on synthetic traces; they are not an
//! A/B experiment with a live agent (that needs a model and is not run here).

use kepler_events::Envelope;
use kepler_focus::{FocusConfig, FocusModule};
use kepler_policy::PolicyEngine;
use std::time::Instant;

#[derive(Clone)]
enum E {
    In(&'static str),
    Call(&'static str),
    Ok,
    Fail,
    Files(&'static [&'static str]),
    Settle,
}

struct Trace {
    name: &'static str,
    events: Vec<E>,
    want: &'static [&'static str],
    forbid: &'static [&'static str],
}

fn env(session: &str, n: usize, kind: &str, payload: serde_json::Value) -> Envelope {
    let json = serde_json::json!({
        "version": 1, "id": format!("e{n}"), "type": kind, "timestamp": "2026-01-02T03:04:05Z",
        "harness": "pi", "project_id": "p", "session_id": session, "correlation_id": "c",
        "payload": payload,
    });
    Envelope::parse(json.to_string().as_bytes()).unwrap()
}

fn run(trace: &Trace, cfg: FocusConfig) -> (Vec<String>, usize, std::time::Duration) {
    let mut e = PolicyEngine::default();
    e.register(Box::new(FocusModule::new(cfg)));
    let mut codes = Vec::new();
    let mut n = 0;
    let mut spent = std::time::Duration::ZERO;
    for ev in &trace.events {
        n += 1;
        let envelope = match ev {
            E::In(t) => env("s", n, "input.received", serde_json::json!({ "text": t })),
            E::Call(c) => env(
                "s",
                n,
                "tool.call.requested",
                serde_json::json!({"tool": "bash", "command": c}),
            ),
            E::Ok | E::Fail => env(
                "s",
                n,
                "tool.call.completed",
                serde_json::json!({"tool": "bash", "ok": matches!(ev, E::Ok), "output_bytes": 1, "duration_ms": 1}),
            ),
            E::Files(p) => env("s", n, "files.changed", serde_json::json!({ "paths": p })),
            E::Settle => env(
                "s",
                n,
                "agent.settled",
                serde_json::json!({"outcome": "ok", "duration_ms": 5}),
            ),
        };
        let t = Instant::now();
        let v = e.evaluate(&envelope);
        spent += t.elapsed();
        codes.extend(v.advisories.into_iter().map(|a| a.code));
    }
    (codes, n, spent)
}

fn traces() -> Vec<Trace> {
    let task = || E::In("refactor the parser in src/parser.rs and keep the public API");
    let mut v = Vec::new();

    // ---- should warn
    v.push(Trace {
        name: "same command over and over, no edits",
        events: [
            vec![task()],
            (0..6)
                .flat_map(|_| [E::Call("cargo test"), E::Fail])
                .collect(),
        ]
        .concat(),
        want: &["loop.repeated_failure"],
        forbid: &[],
    });
    v.push(Trace {
        name: "failure streak with different commands",
        events: {
            let mut e = vec![E::In("make the build pass in src/lib.rs")];
            for c in [
                "cargo build",
                "cargo check",
                "cargo test",
                "cargo clippy",
                "cargo doc",
                "cargo fmt --check",
            ] {
                e.push(E::Call(c));
                e.push(E::Fail);
            }
            e
        },
        want: &["loop.failure_streak"],
        forbid: &[],
    });
    v.push(Trace {
        name: "edit ping-pong between two files",
        events: vec![
            E::In("fix the failing test in src/parser.rs"),
            E::Files(&["src/a.rs"]),
            E::Files(&["src/b.rs"]),
            E::Files(&["src/a.rs"]),
            E::Files(&["src/b.rs"]),
            E::Files(&["src/a.rs"]),
        ],
        want: &["loop.oscillating_changes"],
        forbid: &[],
    });
    v.push(Trace {
        name: "touches many files nobody asked about",
        events: vec![
            E::In("fix the typo in src/parser.rs"),
            E::Files(&[
                "src/parser.rs",
                "src/net/a.rs",
                "src/db/b.rs",
                "web/c.ts",
                "docs/d.md",
            ]),
        ],
        want: &["scope.unrelated_changes"],
        forbid: &[],
    });
    v.push(Trace {
        name: "adds a dependency unasked",
        events: vec![
            E::In("rename a variable in src/parser.rs"),
            E::Files(&["Cargo.toml"]),
        ],
        want: &["minimal.dependency_change"],
        forbid: &[],
    });
    v.push(Trace {
        name: "keeps polishing after tests passed",
        events: {
            let mut e = vec![
                E::In("fix the failing test in src/parser.rs"),
                E::Files(&["src/parser.rs"]),
                E::Call("cargo test"),
                E::Ok,
            ];
            for i in 0..12 {
                e.push(E::Call(
                    ["ls", "cat src/parser.rs", "git diff", "git status"][i % 4],
                ));
                e.push(E::Ok);
            }
            e
        },
        want: &["completion.recommend_stop"],
        forbid: &[],
    });
    v.push(Trace {
        name: "claims done without verifying",
        events: vec![
            E::In("fix the bug in src/parser.rs"),
            E::Files(&["src/parser.rs"]),
            E::Settle,
        ],
        want: &["evidence.unverified_completion"],
        forbid: &["completion.verified"],
    });
    // ---- must stay quiet
    v.push(Trace {
        name: "legitimate edit-test-retry cycles",
        events: {
            let mut e = vec![task_in()];
            for _ in 0..6 {
                e.push(E::Call("cargo test"));
                e.push(E::Fail);
                e.push(E::Files(&["src/parser.rs"]));
            }
            e.push(E::Call("cargo test"));
            e.push(E::Ok);
            e
        },
        want: &[],
        forbid: &[
            "loop.repeated_call",
            "loop.repeated_failure",
            "loop.failure_streak",
            "loop.oscillating_changes",
        ],
    });
    v.push(Trace {
        name: "read-only exploration of a repo",
        events: {
            let mut e = vec![E::In(
                "explain how the parser handles escapes in src/parser.rs",
            )];
            for c in [
                "ls",
                "cat src/parser.rs",
                "grep -rn escape src",
                "cat src/lexer.rs",
                "git log -5",
                "cat README.md",
                "ls src",
                "cat Cargo.toml",
            ] {
                e.push(E::Call(c));
                e.push(E::Ok);
            }
            e
        },
        want: &[],
        forbid: &[
            "loop.repeated_call",
            "budget.stalled",
            "budget.exceeded",
            "completion.recommend_stop",
        ],
    });
    v.push(Trace {
        name: "focused fix with verification",
        events: vec![
            E::In("fix the failing test in src/parser.rs"),
            E::Call("cargo test"),
            E::Fail,
            E::Files(&["src/parser.rs"]),
            E::Call("cargo test"),
            E::Ok,
            E::Settle,
        ],
        want: &["completion.verified"],
        forbid: &[
            "evidence.unverified_completion",
            "loop.repeated_failure",
            "scope.unrelated_changes",
        ],
    });
    v.push(Trace {
        name: "feature touching the files it named",
        events: vec![
            E::In(
                "add a retry option to src/client.rs and src/config.rs and update tests/client.rs",
            ),
            E::Files(&["src/client.rs", "src/config.rs", "tests/client.rs"]),
            E::Call("cargo test"),
            E::Ok,
            E::Settle,
        ],
        want: &["completion.verified"],
        forbid: &["scope.unrelated_changes", "minimal.large_change"],
    });
    v.push(Trace {
        name: "user asks for a dependency",
        events: vec![
            E::In("add the serde dependency to Cargo.toml"),
            E::Files(&["Cargo.toml"]),
        ],
        want: &[],
        forbid: &["minimal.dependency_change"],
    });
    v
}

fn task_in() -> E {
    E::In("fix the failing test in src/parser.rs")
}

#[test]
fn default_thresholds_detect_drift_loops_and_stay_quiet_on_normal_work() {
    let (mut want_total, mut want_hit, mut forbid_hit) = (0, 0, 0);
    let (mut events, mut spent) = (0usize, std::time::Duration::ZERO);
    let mut report = Vec::new();
    for t in traces() {
        let (codes, n, dt) = run(&t, FocusConfig::default());
        events += n;
        spent += dt;
        for w in t.want {
            want_total += 1;
            if codes.iter().any(|c| c == w) {
                want_hit += 1;
            } else {
                report.push(format!("MISS  [{}] wanted {w}, got {codes:?}", t.name));
            }
        }
        for f in t.forbid {
            if codes.iter().any(|c| c == f) {
                forbid_hit += 1;
                report.push(format!("NOISE [{}] forbidden {f}: {codes:?}", t.name));
            }
        }
    }
    let per_event_us = spent.as_secs_f64() * 1e6 / events as f64;
    eprintln!(
        "focus calibration: detected {want_hit}/{want_total}, false positives {forbid_hit}, {per_event_us:.1} us/event over {events} events"
    );
    for r in &report {
        eprintln!("{r}");
    }
    assert!(report.is_empty(), "{}", report.join("\n"));
    if !cfg!(debug_assertions) {
        assert!(per_event_us < 1000.0, "{per_event_us} us/event");
    }
}
