#![allow(clippy::panic, clippy::expect_used)]
//! Rows of `fixtures/forge/prompts.tsv` with a human intent label (3rd column): whatever the
//! rules call *decisive* must agree with the label (a wrong decisive answer can never reach the
//! model), and what they cannot tell must be flagged doubtful.

use kepler_forge::{analyze, Confidence, RuleClass};

const CORPUS: &str = include_str!("../../../fixtures/forge/prompts.tsv");

#[test]
fn decisive_answers_agree_with_human_labels_and_the_rest_is_doubtful() {
    let (mut decisive, mut doubtful, mut wrong) = (0usize, 0usize, Vec::new());
    for line in CORPUS
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let cols: Vec<&str> = line.split('\t').collect();
        let (text, label) = match cols.as_slice() {
            [_, t, l] => (t.replace("\\n", "\n"), *l),
            _ => continue,
        };
        let a = analyze(&text);
        match (a.confidence, a.class()) {
            (Confidence::Doubtful, _) => doubtful += 1,
            (Confidence::Decisive, Some(c)) => {
                decisive += 1;
                if (c == RuleClass::Task) != (label == "task") {
                    wrong.push(format!("{label}: {text:?} ({})", a.reason));
                }
            }
            (Confidence::Decisive, None) => {
                wrong.push(format!("decisive but unclassified: {text:?}"));
            }
        }
    }
    eprintln!("labelled rows: {decisive} decisive by rules, {doubtful} doubtful (model-eligible)");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(decisive >= 15 && doubtful >= 10);
}
