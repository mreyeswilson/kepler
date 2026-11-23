//! Best-effort secret redaction for any free text that might be stored or logged.
//!
//! This is a safety net, not a guarantee: the primary controls are (1) not
//! persisting raw prompts/tool output at all and (2) the sensitive-path denylist.
//! The function is deliberately conservative: it prefers over-redaction.
//!
//! Covered: `key=value` / `key: value` pairs with sensitive key names, CLI flags
//! like `--password x`, `Bearer`/`Basic` credentials, URL userinfo, PEM private key
//! blocks, JWTs, and well-known token prefixes (OpenAI/Stripe, GitHub, Slack, AWS,
//! Google, GitLab, npm), plus long mixed-case high-entropy tokens.

pub const MARK: &str = "[REDACTED]";

const SENSITIVE_KEY_WORDS: &[&str] = &[
    "secret",
    "token",
    "password",
    "passwd",
    "pwd",
    "apikey",
    "api_key",
    "api-key",
    "accesskey",
    "access_key",
    "access-key",
    "private_key",
    "private-key",
    "privatekey",
    "authorization",
    "credential",
    "cookie",
    "passphrase",
];

const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("sk-", 20),
    ("sk_live_", 12),
    ("sk_test_", 12),
    ("rk_live_", 12),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("github_pat_", 20),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xoxa-", 10),
    ("xoxs-", 10),
    ("glpat-", 16),
    ("npm_", 30),
    ("AIza", 30),
];

/// Characters that separate tokens.
fn is_delim(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '{' | '}' | '[' | ']' | '<' | '>'
        )
}

/// Characters that end a value in `key=value`.
fn ends_value(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '}' | ')' | ']' | '&')
}

fn is_sensitive_key(key: &str) -> bool {
    let k = key.trim_start_matches('-').to_lowercase();
    !k.is_empty() && SENSITIVE_KEY_WORDS.iter().any(|w| k.contains(w))
}

/// Redact secrets in `input`. Never returns more text than it was given
/// (apart from the short [`MARK`] replacements).
pub fn redact(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_pem = false;
    for raw in input.split_inclusive('\n') {
        let body = raw.trim_end_matches(['\n', '\r']);
        let ending = &raw[body.len()..];
        if in_pem {
            if body.contains("-----END") {
                in_pem = false;
            }
            continue;
        }
        if body.contains("-----BEGIN") && body.to_uppercase().contains("PRIVATE KEY") {
            out.push_str(MARK);
            out.push_str(ending);
            // An unterminated block swallows the rest of the input (fail safe).
            in_pem = !body.contains("-----END");
            continue;
        }
        out.push_str(&redact_tokens(&redact_pairs(body)));
        out.push_str(ending);
    }
    out
}

/// `key=value`, `key: value`, `"key": "value"` where the key looks sensitive.
fn redact_pairs(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut last = 0usize;
    let mut skip_until = 0usize;
    for (pos, c) in line.char_indices() {
        if pos < skip_until || !(c == '=' || c == ':') {
            continue;
        }
        let before = line[..pos].trim_end_matches(['"', '\'', ' ', '\t']);
        let key_start = before
            .char_indices()
            .rev()
            .find(|(_, ch)| is_delim(*ch))
            .map_or(0, |(p, ch)| p + ch.len_utf8());
        if !is_sensitive_key(&before[key_start..]) {
            continue;
        }
        let mut vs = pos + 1;
        while let Some(ch) = line[vs..].chars().next() {
            if matches!(ch, ' ' | '\t' | '"' | '\'') {
                vs += ch.len_utf8();
            } else {
                break;
            }
        }
        if line[vs..].starts_with(MARK) {
            skip_until = vs + MARK.len();
            continue;
        }
        let mut ve = line[vs..]
            .char_indices()
            .find(|(_, ch)| ends_value(*ch))
            .map_or(line.len(), |(p, _)| vs + p);
        let word = line[vs..ve].to_lowercase();
        if word == "bearer" || word == "basic" {
            // `Authorization: Bearer <token>`: the credential is the next token.
            let rest = &line[ve..];
            let skip = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            let tail = &line[ve + skip..];
            let n = tail
                .char_indices()
                .find(|(_, ch)| ends_value(*ch))
                .map_or(tail.len(), |(p, _)| p);
            ve += skip + n;
        }
        if ve > vs {
            out.push_str(&line[last..vs]);
            out.push_str(MARK);
            last = ve;
            skip_until = ve;
        }
    }
    out.push_str(&line[last..]);
    out
}

fn redact_tokens(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut prev_wants_secret = false;
    let mut chars = line.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if is_delim(c) {
            out.push(c);
            chars.next();
            continue;
        }
        let mut end = line.len();
        while let Some(&(p, ch)) = chars.peek() {
            if is_delim(ch) {
                end = p;
                break;
            }
            chars.next();
        }
        let tok = &line[start..end];
        let lower = tok.to_lowercase();
        if tok.contains(MARK) {
            out.push_str(tok);
            prev_wants_secret = false;
        } else if prev_wants_secret || looks_secret(tok) {
            out.push_str(MARK);
            prev_wants_secret = false;
        } else if let Some(r) = redact_url_userinfo(tok) {
            out.push_str(&r);
            prev_wants_secret = false;
        } else {
            out.push_str(tok);
            prev_wants_secret = lower == "bearer"
                || lower == "basic"
                || (lower.starts_with('-') && !lower.contains('=') && is_sensitive_key(&lower));
        }
    }
    out
}

fn redact_url_userinfo(tok: &str) -> Option<String> {
    let scheme_end = tok.find("://")? + 3;
    let rest = &tok[scheme_end..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..host_end];
    let at = authority.rfind('@')?;
    if at == 0 {
        return None;
    }
    Some(format!("{}{}{}", &tok[..scheme_end], MARK, &rest[at..]))
}

fn looks_secret(tok: &str) -> bool {
    let t = tok.trim_end_matches(['.', '!', '?', ':']);
    if TOKEN_PREFIXES
        .iter()
        .any(|(p, min)| t.starts_with(p) && t.len() >= *min)
    {
        return true;
    }
    // AWS access key ids
    if t.len() == 20
        && (t.starts_with("AKIA") || t.starts_with("ASIA"))
        && t.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return true;
    }
    // JWT: base64url header starting with eyJ and three dot-separated parts
    if t.starts_with("eyJ") && t.matches('.').count() == 2 && t.len() > 30 {
        return true;
    }
    // High-entropy blob: long, base64-ish alphabet, mixed case and digits.
    if t.len() >= 32
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'_' | b'-' | b'='))
    {
        let upper = t.bytes().any(|b| b.is_ascii_uppercase());
        let lower = t.bytes().any(|b| b.is_ascii_lowercase());
        let digit = t.bytes().any(|b| b.is_ascii_digit());
        return upper && lower && digit;
    }
    false
}

/// Names of the kinds of secret found in `text` (empty = nothing found). Kinds are coarse
/// categories, never the secret itself. Built on [`redact`]: whatever it would remove counts.
pub fn detect_secrets(text: &str) -> Vec<&'static str> {
    if redact(text) == text {
        return Vec::new();
    }
    let probes: &[(&str, &str)] = &[
        ("-----BEGIN", "private_key"),
        ("AKIA", "aws_access_key"),
        ("ASIA", "aws_access_key"),
        ("ghp_", "github_token"),
        ("github_pat_", "github_token"),
        ("gho_", "github_token"),
        ("glpat-", "gitlab_token"),
        ("xox", "slack_token"),
        ("sk-", "api_key"),
        ("AIza", "google_api_key"),
        ("npm_", "npm_token"),
        ("eyJ", "jwt"),
        ("Bearer ", "bearer_credential"),
        ("Basic ", "basic_credential"),
    ];
    let mut kinds: Vec<&'static str> = Vec::new();
    for (needle, kind) in probes {
        if text.contains(needle) && !kinds.contains(kind) {
            kinds.push(kind);
        }
    }
    if kinds.is_empty() {
        kinds.push("credential_pattern");
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_secrets_names_kinds_without_leaking() {
        assert!(detect_secrets("plain output: 3 tests passed").is_empty());
        assert!(detect_secrets("src/main.rs:10: error[E0308]").is_empty());
        let k = detect_secrets(
            "-----BEGIN RSA PRIVATE KEY-----
abc
-----END RSA PRIVATE KEY-----",
        );
        assert_eq!(k, vec!["private_key"]);
        assert!(detect_secrets("aws AKIAIOSFODNN7EXAMPLE here").contains(&"aws_access_key"));
        assert!(
            detect_secrets("token ghp_abcdefghijklmnopqrstuvwxyz0123456789")
                .contains(&"github_token")
        );
        assert!(detect_secrets("DB_PASSWORD=hunter2").contains(&"credential_pattern"));
        for kind in detect_secrets("export API_KEY=abc123xyz") {
            assert!(!kind.contains("abc123"));
        }
    }

    fn gone(input: &str, secret: &str) {
        let out = redact(input);
        assert!(
            !out.contains(secret),
            "leaked `{secret}` in `{out}` (from `{input}`)"
        );
        assert!(out.contains(MARK), "no marker in `{out}`");
    }

    #[test]
    fn key_value_pairs() {
        gone("DB_PASSWORD=hunter2", "hunter2");
        gone("export API_KEY=abc123xyz", "abc123xyz");
        gone("password: s3cr3t!", "s3cr3t!");
        gone(
            r#"{"client_secret": "zzz-top-secret", "user": "bob"}"#,
            "zzz-top-secret",
        );
        gone("token = 'abcdef'", "abcdef");
        gone(
            "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "wJalrXUtnFEMI",
        );
    }

    #[test]
    fn pair_redaction_keeps_other_fields() {
        let out = redact(r#"{"password": "hunter2", "user": "bob"}"#);
        assert!(out.contains("\"user\": \"bob\""), "{out}");
        assert!(!out.contains("hunter2"));
        let out = redact("PORT=8080 TOKEN=abc HOST=localhost");
        assert!(
            out.contains("PORT=8080") && out.contains("HOST=localhost"),
            "{out}"
        );
    }

    #[test]
    fn cli_flags_and_bearer() {
        gone("curl --password hunter2 http://x", "hunter2");
        gone(
            "curl -H 'Authorization: Bearer abc.def.ghi' http://x",
            "abc.def.ghi",
        );
        gone("Authorization: Basic dXNlcjpwYXNz", "dXNlcjpwYXNz");
    }

    #[test]
    fn known_token_prefixes() {
        for t in [
            "sk-proj-abcdefghijklmnopqrstuvwx",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_11ABCDEFG0abcdefghijklmnop",
            "xoxb-123456789012-abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "AIzaSyA-abcdefghijklmnopqrstuvwxyz01234",
            "glpat-abcdefghijklmnopqrstu",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
        ] {
            gone(&format!("here is the thing {t} ok"), t);
        }
    }

    #[test]
    fn pem_blocks_removed() {
        let pem = "before\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nabcdef\n-----END RSA PRIVATE KEY-----\nafter\n";
        let out = redact(pem);
        assert!(!out.contains("MIIEow") && !out.contains("abcdef"), "{out}");
        assert!(out.contains("before") && out.contains("after"));
        // unterminated block swallows the rest
        let out = redact("x\n-----BEGIN PRIVATE KEY-----\nAAAA\nBBBB\n");
        assert!(!out.contains("AAAA") && !out.contains("BBBB"));
    }

    #[test]
    fn url_userinfo() {
        let out = redact("git clone https://bob:hunter2@github.com/x/y.git");
        assert!(!out.contains("hunter2") && !out.contains("bob:"), "{out}");
        assert!(out.contains("github.com/x/y.git"));
    }

    #[test]
    fn high_entropy_blob() {
        gone(
            "key is Zm9vYmFyMTIzNDU2Nzg5MEFCQ0RFRkdISUpLTE1OTw== done",
            "Zm9vYmFyMTIz",
        );
    }

    #[test]
    fn ordinary_text_is_untouched() {
        for t in [
            "Fix the failing test in parser.rs",
            "cargo test --workspace",
            "commit 5e39b57d34f0f0c1a2b3c4d5e6f708192a3b4c5d fixes it",
            "see https://example.com/docs/page?x=1 for details",
            "the author: Jane wrote src/lib.rs",
            "time 12:30:45 and ratio 3:1",
            "multi\nline\r\ntext\n",
        ] {
            assert_eq!(redact(t), t, "{t}");
        }
    }

    #[test]
    fn preserves_line_structure() {
        let out = redact("a=1\nPASSWORD=x\nb=2\n");
        assert_eq!(out.lines().count(), 3);
        assert!(out.contains("a=1") && out.contains("b=2"));
    }

    #[test]
    fn is_idempotent() {
        let once = redact("PASSWORD=hunter2 and sk-abcdefghijklmnopqrstuvwx");
        assert_eq!(redact(&once), once);
    }

    #[test]
    fn unicode_does_not_panic() {
        let _ = redact("contraseña: ñandú 😀 token=ü😀ü\n¿qué?: password:😀");
    }
}
