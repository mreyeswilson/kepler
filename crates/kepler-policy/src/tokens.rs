//! Token accounting with an explicit label: a number is either **estimated** by Kepler
//! (a heuristic, never exact) or **measured** by the harness. The two are never mixed
//! in one figure; consumers must carry the [`Basis`] next to the number.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Basis {
    /// Kepler's heuristic. Accuracy depends on the model's tokenizer, which we do not run.
    Estimated,
    /// Reported by the harness/provider (usage fields, tokenizer calls).
    Measured,
}

impl Basis {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Estimated => "estimated",
            Self::Measured => "measured",
        }
    }
    pub fn is_estimated(self) -> bool {
        self == Self::Estimated
    }
}

/// Rough tokenizer families (no tokenizer is bundled; see ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ModelFamily {
    #[default]
    Generic,
    Claude,
    Gpt,
    Gemini,
    Local,
}

impl ModelFamily {
    /// Best-effort mapping from a model name; unknown names are `Generic`.
    pub fn from_model(name: &str) -> Self {
        let n = name.to_lowercase();
        if n.contains("claude") || n.contains("sonnet") || n.contains("opus") || n.contains("haiku")
        {
            Self::Claude
        } else if n.contains("gpt") || n.starts_with("o1") || n.starts_with("o3") {
            Self::Gpt
        } else if n.contains("gemini") {
            Self::Gemini
        } else if n.contains("llama") || n.contains("qwen") || n.contains("mistral") {
            Self::Local
        } else {
            Self::Generic
        }
    }

    /// Average ASCII characters per token (conservative: lower = more tokens).
    fn chars_per_token(self) -> f64 {
        match self {
            Self::Claude => 3.5,
            Self::Generic | Self::Gpt | Self::Gemini => 4.0,
            Self::Local => 3.2,
        }
    }
}

/// A token figure with its basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCount {
    pub tokens: u32,
    pub basis: Basis,
}

impl TokenCount {
    pub fn estimated(tokens: u32) -> Self {
        Self {
            tokens,
            basis: Basis::Estimated,
        }
    }
    pub fn measured(tokens: u32) -> Self {
        Self {
            tokens,
            basis: Basis::Measured,
        }
    }
}

/// Estimate the tokens of `text`: ASCII at the family's chars-per-token ratio, non-ASCII
/// (CJK, emoji, accents) at ~1.5 chars per token. Always labeled [`Basis::Estimated`].
pub fn estimate_tokens(text: &str, family: ModelFamily) -> TokenCount {
    let (mut ascii, mut other) = (0usize, 0usize);
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            other += 1;
        }
    }
    let t = ascii as f64 / family.chars_per_token() + other as f64 / 1.5;
    TokenCount::estimated(t.ceil().min(f64::from(u32::MAX)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_are_labeled_and_monotonic() {
        let a = estimate_tokens("hello world", ModelFamily::Generic);
        let b = estimate_tokens(&"hello world ".repeat(100), ModelFamily::Generic);
        assert_eq!(a.basis, Basis::Estimated);
        assert!(b.tokens > a.tokens * 50);
        assert_eq!(estimate_tokens("", ModelFamily::Claude).tokens, 0);
        assert!(a.basis.is_estimated());
        assert_ne!(Basis::Measured, a.basis);
    }

    #[test]
    fn non_ascii_costs_more_per_char() {
        let ascii = estimate_tokens(&"a".repeat(30), ModelFamily::Generic).tokens;
        let cjk = estimate_tokens(&"日".repeat(30), ModelFamily::Generic).tokens;
        assert!(cjk > ascii * 2, "{cjk} vs {ascii}");
    }

    #[test]
    fn family_mapping() {
        assert_eq!(
            ModelFamily::from_model("claude-sonnet-4"),
            ModelFamily::Claude
        );
        assert_eq!(ModelFamily::from_model("GPT-5"), ModelFamily::Gpt);
        assert_eq!(ModelFamily::from_model("something"), ModelFamily::Generic);
        assert!(
            estimate_tokens("some code here", ModelFamily::Claude).tokens
                >= estimate_tokens("some code here", ModelFamily::Generic).tokens
        );
    }

    #[test]
    fn measured_is_a_distinct_basis() {
        assert_eq!(TokenCount::measured(5).basis, Basis::Measured);
        assert_ne!(TokenCount::measured(5), TokenCount::estimated(5));
    }
}
