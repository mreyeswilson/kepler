/// Deterministic, local classification of a user input. No model calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputClass {
    /// Nothing to enrich: acknowledgements, greetings, slash commands, empty input.
    Trivial(&'static str),
    /// Anything else. D0 has no enrichment modules, so this still passes through.
    Substantive,
}

const ACKS: &[&str] = &[
    "y",
    "n",
    "yes",
    "no",
    "ok",
    "okay",
    "k",
    "thanks",
    "thank you",
    "thx",
    "continue",
    "go on",
    "go",
    "proceed",
    "stop",
    "done",
    "hi",
    "hello",
    "hey",
    "sure",
    "yep",
    "nope",
    "si",
    "sí",
    "gracias",
    "hola",
    "dale",
    "vale",
    "listo",
    "continua",
    "continúa",
    "sigue",
    "adelante",
];

pub fn classify_input(text: &str) -> InputClass {
    let t = text.trim();
    if t.is_empty() {
        return InputClass::Trivial("empty");
    }
    if t.starts_with('/') && !t.contains(char::is_whitespace) || t.starts_with('!') {
        return InputClass::Trivial("command_like");
    }
    let normalized: String = t
        .trim_end_matches(|c: char| c.is_ascii_punctuation() || c == '¡' || c == '¿')
        .trim_start_matches(['¡', '¿'])
        .to_lowercase();
    if ACKS.contains(&normalized.as_str()) {
        return InputClass::Trivial("acknowledgement");
    }
    if t.chars().count() <= 3 {
        return InputClass::Trivial("too_short");
    }
    InputClass::Substantive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_inputs() {
        for t in [
            "", "   ", "ok", "OK!", "Thanks.", "yes", "sí", "¡Hola!", "/help", "!ls", "go on", "?!",
        ] {
            assert!(matches!(classify_input(t), InputClass::Trivial(_)), "{t:?}");
        }
    }

    #[test]
    fn substantive_inputs() {
        for t in [
            "Fix the failing test in parser.rs",
            "/review the diff and list risks",
            "why is the build slow",
            "ok but also refactor the parser",
        ] {
            assert_eq!(classify_input(t), InputClass::Substantive, "{t:?}");
        }
    }
}
