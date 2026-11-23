//! Guardian bypass corpus (`fixtures/guardian/bypass.txt`): every line states what the
//! preflight must answer. A regression here is a security regression.

use kepler_events::ToolCallRequested;
use kepler_policy::{preflight_tool_call, Decision};

const CORPUS: &str = include_str!("../../../fixtures/guardian/bypass.txt");

fn verdict(cmd: &str) -> Decision {
    preflight_tool_call(&ToolCallRequested {
        tool: "bash".into(),
        command: Some(cmd.into()),
        paths: vec![],
        destructive: false,
    })
}

#[test]
fn corpus_matches_expectations() {
    let mut failures = Vec::new();
    let mut counts = [0usize; 3];
    for (n, line) in CORPUS.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((expect, cmd)) = line.split_once('\t') else {
            failures.push(format!("line {}: malformed", n + 1));
            continue;
        };
        let got = verdict(cmd);
        let ok = match expect {
            "deny" => {
                counts[0] += 1;
                matches!(got, Decision::Deny { .. })
            }
            "approve" => {
                counts[1] += 1;
                matches!(
                    got,
                    Decision::RequireApproval { .. } | Decision::Deny { .. }
                )
            }
            "pass" => {
                counts[2] += 1;
                got.is_passthrough()
            }
            other => {
                failures.push(format!("line {}: unknown expectation {other}", n + 1));
                continue;
            }
        };
        if !ok {
            failures.push(format!(
                "line {}: expected {expect} for `{cmd}`, got {got:?}",
                n + 1
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        counts[0] >= 40 && counts[1] >= 90 && counts[2] >= 25,
        "{counts:?}"
    );
}

/// Cost of the preflight on the whole corpus (printed; asserted only loosely so debug builds
/// and slow CI machines do not flake).
#[test]
fn preflight_cost_is_small() {
    let cmds: Vec<&str> = CORPUS
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('\t').map(|(_, c)| c))
        .collect();
    let t = std::time::Instant::now();
    let rounds = 5;
    for _ in 0..rounds {
        for c in &cmds {
            let _ = verdict(c);
        }
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / (rounds * cmds.len()) as f64;
    eprintln!(
        "guardian preflight: {per:.1} us/command over {} commands",
        cmds.len()
    );
    assert!(per < 20_000.0, "{per} us/command");
}

#[test]
fn tool_result_secrets_are_redacted_not_echoed() {
    use kepler_policy::guardian::Guardian;
    let done = kepler_events::ToolCallCompleted {
        tool: "bash".into(),
        ok: true,
        output_bytes: 40,
        duration_ms: 1,
        output_excerpt: Some("API=ghp_abcdefghijklmnopqrstuvwxyz0123456789".into()),
    };
    match Guardian::default().scan_result(&done) {
        Decision::Redact { kinds, .. } => assert!(!kinds.is_empty()),
        d => panic!("{d:?}"),
    }
}
