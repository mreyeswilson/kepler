//! Explicit, user-authored memory directives (es/en). Deterministic: a fixed list of lowercase
//! prefixes matched at the start of a sentence. Nothing is inferred; the stored text is what
//! the user wrote (redaction happens at store time).

const MAX_DIRECTIVE_BYTES: usize = 500;
const MAX_DIRECTIVES: usize = 3;

/// "remember that X": store only X.
const STRIP_PREFIXES: &[&str] = &[
    "remember that ",
    "remember: ",
    "please remember that ",
    "recuerda que ",
    "recuerda: ",
    "recuerda siempre que ",
    "acuerdate de que ",
    "ten en cuenta que ",
    "keep in mind that ",
];

/// Standing-instruction openers: store the whole sentence (the instruction is the memory).
const KEEP_PREFIXES: &[&str] = &[
    "a partir de ahora ",
    "de ahora en adelante ",
    "from now on ",
    "going forward ",
    "siempre usa ",
    "siempre utiliza ",
    "siempre ejecuta ",
    "siempre prefiere ",
    "siempre pasa ",
    "nunca hagas ",
    "nunca uses ",
    "nunca utilices ",
    "nunca toques ",
    "nunca borres ",
    "nunca ejecutes ",
    "nunca commitees ",
    "no uses nunca ",
    "no hagas nunca ",
    "always use ",
    "always run ",
    "always prefer ",
    "always pass ",
    "never do ",
    "never use ",
    "never touch ",
    "never delete ",
    "never run ",
    "never commit ",
    "don't ever ",
    "do not ever ",
];

/// Leading filler that may precede a trigger ("y a partir de ahora ...", "ok, siempre usa ...").
const FILLERS: &[&str] = &[
    "y ",
    "and ",
    "also, ",
    "also ",
    "ok, ",
    "ok ",
    "vale, ",
    "por favor ",
    "please ",
];

fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for line in text.lines() {
        let b = line.as_bytes();
        let mut start = 0;
        for i in 0..b.len() {
            let end_here = matches!(b[i], b'.' | b'!' | b';')
                && (i + 1 == b.len() || b[i + 1].is_ascii_whitespace());
            if end_here {
                out.push(line[start..=i].trim());
                start = i + 1;
            }
        }
        if start < line.len() {
            out.push(line[start..].trim());
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

fn one(sentence: &str) -> Option<String> {
    let mut s = sentence.trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '-' | '*' | '>' | '"' | '\'')
    });
    if s.ends_with('?') || s.contains('\0') {
        return None;
    }
    // Case folding can change byte lengths for some scripts: refuse rather than mis-slice.
    let mut lower = s.to_lowercase();
    if lower.len() != s.len() {
        return None;
    }
    while let Some(f) = FILLERS.iter().find(|f| lower.starts_with(**f)) {
        s = s[f.len()..].trim_start();
        lower = s.to_lowercase();
        if lower.len() != s.len() {
            return None;
        }
    }
    for p in STRIP_PREFIXES {
        if lower.starts_with(p) {
            return ok_len(&s[p.len()..]);
        }
    }
    for p in KEEP_PREFIXES {
        if lower.starts_with(p) && !s[p.len()..].trim().is_empty() {
            return ok_len(s);
        }
    }
    None
}

fn ok_len(s: &str) -> Option<String> {
    let t = s.trim();
    (t.chars().count() >= 3 && t.len() <= MAX_DIRECTIVE_BYTES).then(|| t.to_string())
}

/// Directives the user stated explicitly in `text` (at most 3, in order, deduplicated).
pub fn user_directives(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in sentences(text) {
        if let Some(d) = one(s) {
            if !out.contains(&d) {
                out.push(d);
            }
        }
        if out.len() >= MAX_DIRECTIVES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_phrases_es_en() {
        assert_eq!(
            user_directives("Recuerda que usamos pnpm y no npm"),
            vec!["usamos pnpm y no npm"]
        );
        assert_eq!(
            user_directives("Remember that we use pnpm, not npm."),
            vec!["we use pnpm, not npm."]
        );
        assert_eq!(
            user_directives("A partir de ahora responde en español"),
            vec!["A partir de ahora responde en español"]
        );
        assert_eq!(
            user_directives("Fix the bug. Also, from now on run tests with nextest."),
            vec!["from now on run tests with nextest."]
        );
        assert_eq!(
            user_directives("siempre usa pnpm\nnunca toques el directorio dist"),
            vec!["siempre usa pnpm", "nunca toques el directorio dist"]
        );
        assert_eq!(
            user_directives("y a partir de ahora usa tabs"),
            vec!["a partir de ahora usa tabs"]
        );
    }

    #[test]
    fn not_directives() {
        for no in [
            "I remember that we use pnpm",
            "should I remember that?",
            "remember that ",
            "refactor the parser",
            "¿siempre usa pnpm este proyecto?",
            "the user never use tabs",
            "a partir de",
            "",
        ] {
            assert!(user_directives(no).is_empty(), "{no:?}");
        }
        assert!(user_directives(&format!("remember that {}", "x".repeat(2000))).is_empty());
        let many = "recuerda que a1.\nrecuerda que b2.\nrecuerda que c3.\nrecuerda que d4.";
        assert_eq!(user_directives(many).len(), 3);
    }
}
