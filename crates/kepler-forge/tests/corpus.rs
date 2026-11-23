#![allow(clippy::panic, clippy::expect_used)]
//! Forge classifier accuracy on `fixtures/forge/prompts.tsv` (hand-labelled, es/en).
//! Prints a confusion summary; fails below the thresholds recorded in docs.

use kepler_forge::{analyze, Mode};

const CORPUS: &str = include_str!("../../../fixtures/forge/prompts.tsv");

fn parse(m: &str) -> Mode {
    match m {
        "passthrough" => Mode::Passthrough,
        "enrich" => Mode::Enrich,
        "compile" => Mode::Compile,
        "decompose" => Mode::Decompose,
        other => panic!("bad mode {other}"),
    }
}

#[test]
fn corpus_accuracy_and_invariants() {
    let (mut total, mut ok, mut doubtful) = (0usize, 0usize, 0usize);
    let mut per: std::collections::BTreeMap<&str, (usize, usize)> = Default::default();
    let mut misses = Vec::new();
    for line in CORPUS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let (exp, rest) = line.split_once('\t').expect("tab");
        let text = rest.split('\t').next().unwrap_or("").replace("\\n", "\n");
        let a = analyze(&text);
        // Invariant 1: the original is preserved byte for byte.
        assert_eq!(a.original(), text);
        // Invariant 2: passthrough carries no derived content.
        if a.mode == Mode::Passthrough {
            assert!(a.spec.is_empty(), "passthrough with spec: {text}");
        }
        if a.confidence == kepler_forge::Confidence::Doubtful {
            doubtful += 1;
        }
        let e = per.entry(exp).or_default();
        e.0 += 1;
        total += 1;
        if a.mode == parse(exp) {
            ok += 1;
            e.1 += 1;
        } else {
            misses.push(format!(
                "expected {exp}, got {} ({}) : {text:?}",
                a.mode.as_str(),
                a.reason
            ));
        }
    }
    for (k, (n, h)) in &per {
        eprintln!("{k:12} {h}/{n}");
    }
    for m in &misses {
        eprintln!("MISS {m}");
    }
    let acc = ok as f64 / total as f64;
    eprintln!("accuracy {ok}/{total} = {:.1}%", acc * 100.0);
    eprintln!(
        "model calls avoided by the rules: {}/{total} ({doubtful} doubtful)",
        total - doubtful
    );
    assert!(acc >= 0.95, "accuracy {acc:.3}");
    // Never mutate trivia or questions into work: precision of non-passthrough modes.
    let false_enrich = misses
        .iter()
        .filter(|m| m.starts_with("expected passthrough"))
        .count();
    assert!(
        false_enrich <= 3,
        "too many unwanted enrichments:\n{}",
        misses.join("\n")
    );
}
