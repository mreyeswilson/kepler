/// An explicit user request to remember something ("remember that we use pnpm").
/// Only an explicit directive at the start of the message counts; nothing is guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDirective {
    /// The remembered text, verbatim from the user (trimmed).
    pub text: String,
}

const PREFIXES: &[&str] = &[
    "remember that ",
    "remember: ",
    "please remember that ",
    "recuerda que ",
    "recuerda: ",
    "por favor recuerda que ",
    "kepler remember: ",
];
const MAX_DIRECTIVE_BYTES: usize = 1024;

pub fn memory_directive(text: &str) -> Option<MemoryDirective> {
    let t = text.trim_start();
    let lower = t.to_lowercase();
    // `to_lowercase` can change byte lengths for some scripts; only ASCII prefixes are used,
    // so require the lowercase form and the original to agree on the prefix length.
    for p in PREFIXES {
        if lower.starts_with(p)
            && t.is_char_boundary(p.len())
            && t[..p.len()].eq_ignore_ascii_case(p)
        {
            let rest = t[p.len()..].trim();
            if rest.is_empty() || rest.len() > MAX_DIRECTIVE_BYTES {
                return None;
            }
            return Some(MemoryDirective {
                text: rest.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_directives_only() {
        assert_eq!(
            memory_directive("Remember that we use pnpm, not npm.")
                .unwrap()
                .text,
            "we use pnpm, not npm."
        );
        assert_eq!(
            memory_directive("  recuerda que el CI corre en Windows")
                .unwrap()
                .text,
            "el CI corre en Windows"
        );
        for no in [
            "I remember that we use pnpm",
            "should I remember that?",
            "remember that ",
            "refactor the parser",
            "",
        ] {
            assert!(memory_directive(no).is_none(), "{no:?}");
        }
        assert!(memory_directive(&format!("remember that {}", "x".repeat(2000))).is_none());
    }
}
