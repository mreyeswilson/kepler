//! Deterministic rules. A rule is a fixed substring match on added text (never a regex over
//! untrusted input), a path predicate, or a count. Suspected findings are heuristics.

use crate::diff::FileDiff;
use kepler_policy::redact::detect_secrets;
use kepler_policy::sensitive::is_sensitive_path;
use kepler_storage::{Confidence, NewFinding, Severity};

const MAX_PER_FILE: usize = 20;
const MAX_TOTAL: usize = 500;
const LARGE_ADDED: usize = 1000;

/// A rule match before it is attached to a project/session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub rule_id: &'static str,
    pub severity: Severity,
    pub confidence: Confidence,
    pub path: String,
    pub line: Option<u32>,
    pub message: String,
    pub evidence: String,
}

impl Hit {
    pub fn into_finding(self, project_id: &str, session_id: &str) -> NewFinding {
        NewFinding {
            project_id: project_id.into(),
            session_id: session_id.into(),
            source: "sentinel".into(),
            rule_id: self.rule_id.into(),
            severity: self.severity,
            confidence: self.confidence,
            path: self.path,
            line: self.line,
            message: self.message,
            evidence: self.evidence,
        }
    }
}

struct LineRule {
    id: &'static str,
    severity: Severity,
    /// Lowercase file extensions this applies to; empty = any text file.
    exts: &'static [&'static str],
    /// Case-sensitive substrings; any one matches.
    needles: &'static [&'static str],
    message: &'static str,
    /// Skip test files (unwrap/console.log are normal there).
    skip_tests: bool,
}

const RS: &[&str] = &["rs"];
const JS: &[&str] = &["js", "jsx", "ts", "tsx", "mjs", "cjs"];
const PY: &[&str] = &["py"];

const LINE_RULES: &[LineRule] = &[
    LineRule {
        id: "rust.unsafe",
        severity: Severity::Medium,
        exts: RS,
        needles: &["unsafe {", "unsafe fn", "unsafe impl"],
        message: "New unsafe code; needs a documented safety argument.",
        skip_tests: false,
    },
    LineRule {
        id: "rust.unwrap",
        severity: Severity::Low,
        exts: RS,
        needles: &[".unwrap()", ".expect("],
        message: "unwrap/expect in non-test code can panic.",
        skip_tests: true,
    },
    LineRule {
        id: "rust.todo",
        severity: Severity::Low,
        exts: RS,
        needles: &["todo!(", "unimplemented!("],
        message: "Placeholder that panics at runtime.",
        skip_tests: true,
    },
    LineRule {
        id: "rust.ignored_test",
        severity: Severity::Medium,
        exts: RS,
        needles: &["#[ignore"],
        message: "A test was disabled.",
        skip_tests: false,
    },
    LineRule {
        id: "rust.dbg",
        severity: Severity::Low,
        exts: RS,
        needles: &["dbg!("],
        message: "Debug macro left in code.",
        skip_tests: true,
    },
    LineRule {
        id: "js.eval",
        severity: Severity::High,
        exts: JS,
        needles: &["eval(", "new Function("],
        message: "Dynamic code evaluation.",
        skip_tests: false,
    },
    LineRule {
        id: "js.focused_test",
        severity: Severity::Medium,
        exts: JS,
        needles: &[".only(", "fdescribe(", "fit("],
        message: "Focused test: the rest of the suite is silently skipped.",
        skip_tests: false,
    },
    LineRule {
        id: "js.debugger",
        severity: Severity::Low,
        exts: JS,
        needles: &["debugger;", "debugger\n"],
        message: "debugger statement left in code.",
        skip_tests: false,
    },
    LineRule {
        id: "js.inner_html",
        severity: Severity::Medium,
        exts: JS,
        needles: &[".innerHTML =", ".innerHTML=", "dangerouslySetInnerHTML"],
        message: "Raw HTML injection point (XSS risk if the value is untrusted).",
        skip_tests: true,
    },
    LineRule {
        id: "js.console_log",
        severity: Severity::Info,
        exts: JS,
        needles: &["console.log("],
        message: "console.log left in code.",
        skip_tests: true,
    },
    LineRule {
        id: "py.eval",
        severity: Severity::High,
        exts: PY,
        needles: &["eval(", "exec("],
        message: "Dynamic code evaluation.",
        skip_tests: true,
    },
    LineRule {
        id: "py.shell_true",
        severity: Severity::Medium,
        exts: PY,
        needles: &["shell=True"],
        message: "Subprocess through a shell (injection risk).",
        skip_tests: true,
    },
    LineRule {
        id: "py.pickle",
        severity: Severity::Medium,
        exts: PY,
        needles: &["pickle.load", "yaml.load("],
        message: "Unsafe deserialization of untrusted data.",
        skip_tests: true,
    },
    LineRule {
        id: "tls.verify_disabled",
        severity: Severity::High,
        exts: &[],
        needles: &[
            "verify=False",
            "rejectUnauthorized: false",
            "NODE_TLS_REJECT_UNAUTHORIZED",
            "InsecureSkipVerify",
            "danger_accept_invalid_certs",
        ],
        message: "TLS certificate verification disabled.",
        skip_tests: true,
    },
    LineRule {
        id: "shell.curl_pipe",
        severity: Severity::High,
        exts: &[],
        needles: &["| sh", "| bash", "|sh\n", "| sudo sh", "| sudo bash"],
        message: "Piping downloaded content into a shell.",
        skip_tests: false,
    },
    LineRule {
        id: "shell.chmod_777",
        severity: Severity::High,
        exts: &[],
        needles: &["chmod 777", "chmod -R 777", "chmod a+rwx"],
        message: "World-writable permissions.",
        skip_tests: false,
    },
    LineRule {
        id: "shell.no_verify",
        severity: Severity::Medium,
        exts: &[],
        needles: &["--no-verify", "--no-gpg-sign"],
        message: "Bypasses git hooks or signing.",
        skip_tests: false,
    },
    LineRule {
        id: "shell.rm_rf",
        severity: Severity::Medium,
        exts: &[
            "sh",
            "bash",
            "ps1",
            "yml",
            "yaml",
            "mk",
            "makefile",
            "dockerfile",
        ],
        needles: &["rm -rf /", "rm -rf $", "Remove-Item -Recurse -Force $"],
        message: "Recursive delete driven by a variable or root path.",
        skip_tests: false,
    },
    LineRule {
        id: "marker.fixme",
        severity: Severity::Info,
        exts: &[],
        needles: &["FIXME", "HACK", "XXX"],
        message: "Unresolved marker added.",
        skip_tests: true,
    },
];

const CRED_WORDS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "api_key",
    "apikey",
    "auth_token",
];
const CRED_SAFE: &[&str] = &[
    "env",
    "getenv",
    "environ",
    "example",
    "placeholder",
    "your_",
    "xxx",
    "<",
    "{{",
    "${",
    "todo",
    "changeme",
    "dummy",
    "fake",
    "test",
];

fn ext_of(path: &str) -> String {
    let base = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_lowercase();
    match base.rsplit_once('.') {
        Some((_, e)) => e.to_string(),
        None => base,
    }
}

fn is_test_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_lowercase();
    p.starts_with("tests/")
        || p.starts_with("test/")
        || p.contains("/tests/")
        || p.contains("/test/")
        || p.contains("__tests__")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("_test.")
        || p.contains("/fixtures/")
}

/// Quoted literal of plausible secret length on a line that names a credential.
fn hardcoded_credential(line: &str) -> bool {
    let lower = line.to_lowercase();
    if !CRED_WORDS.iter().any(|w| lower.contains(w)) || CRED_SAFE.iter().any(|w| lower.contains(w))
    {
        return false;
    }
    if !(line.contains('=') || line.contains(':')) {
        return false;
    }
    for q in ['"', '\''] {
        let mut parts = line.split(q).skip(1).step_by(2);
        if parts
            .any(|lit| (8..=128).contains(&lit.len()) && !lit.contains(' ') && !lit.contains('/'))
        {
            return true;
        }
    }
    false
}

/// Rules over added lines of one file.
fn scan_file(f: &FileDiff, out: &mut Vec<Hit>) {
    let ext = ext_of(&f.path);
    let test = is_test_path(&f.path);
    let start = out.len();
    let full = |out: &mut Vec<Hit>, h: Hit| {
        if out.len() - start < MAX_PER_FILE {
            out.push(h);
        }
    };
    for (line_no, text) in &f.added {
        let kinds = detect_secrets(text);
        // `credential_pattern` is the redactor's generic `name = value` fallback: a heuristic, so
        // it is only suspected, and reads from the environment are not reported at all.
        let generic = kinds == ["credential_pattern"];
        let env_read = {
            let l = text.to_lowercase();
            CRED_SAFE.iter().any(|w| l.contains(w))
        };
        if !kinds.is_empty() && !(generic && (env_read || test)) {
            full(
                out,
                Hit {
                    rule_id: if generic {
                        "diff.credential_assignment"
                    } else {
                        "diff.secret"
                    },
                    severity: if generic {
                        Severity::High
                    } else {
                        Severity::Critical
                    },
                    confidence: if generic {
                        Confidence::Suspected
                    } else {
                        Confidence::Confirmed
                    },
                    path: f.path.clone(),
                    line: Some(*line_no),
                    message: if generic {
                        "A credential-named variable is assigned a value. Remove it if real."
                    } else {
                        "A secret-shaped value was added. Remove it and rotate the credential."
                    }
                    .into(),
                    // The value itself is never copied.
                    evidence: format!("secret kinds: {}", kinds.join(",")),
                },
            );
            continue; // do not echo a line that holds a secret through other rules
        }
        if !test && hardcoded_credential(text) {
            full(
                out,
                Hit {
                    rule_id: "diff.hardcoded_credential",
                    severity: Severity::Medium,
                    confidence: Confidence::Suspected,
                    path: f.path.clone(),
                    line: Some(*line_no),
                    message: "A credential-named variable is assigned a literal.".into(),
                    evidence: "credential-like literal (value withheld)".into(),
                },
            );
            continue;
        }
        for r in LINE_RULES {
            if (!r.exts.is_empty() && !r.exts.contains(&ext.as_str())) || (r.skip_tests && test) {
                continue;
            }
            if r.needles.iter().any(|n| text.contains(n)) {
                full(
                    out,
                    Hit {
                        rule_id: r.id,
                        severity: r.severity,
                        confidence: Confidence::Suspected,
                        path: f.path.clone(),
                        line: Some(*line_no),
                        message: r.message.into(),
                        evidence: text.trim().to_string(),
                    },
                );
            }
        }
    }
    let is_marker = |l: &str| {
        let t = l.trim_start();
        t.starts_with("#[test]")
            || t.starts_with("def test_")
            || t.starts_with("it(")
            || t.starts_with("test(")
            || t.starts_with("fn test_")
    };
    let removed = f.removed.iter().filter(|l| is_marker(l)).count();
    let added = f.added.iter().filter(|(_, l)| is_marker(l)).count();
    if removed >= 3 && added * 2 < removed {
        full(
            out,
            Hit {
                rule_id: "diff.tests_removed",
                severity: Severity::Medium,
                confidence: Confidence::Suspected,
                path: f.path.clone(),
                line: None,
                message: format!("{removed} test markers removed, {added} added."),
                evidence: format!("removed={removed} added={added}"),
            },
        );
    }
    if f.added.len() > LARGE_ADDED {
        full(
            out,
            Hit {
                rule_id: "diff.large",
                severity: Severity::Info,
                confidence: Confidence::Suspected,
                path: f.path.clone(),
                line: None,
                message: format!("{} lines added in one file; hard to review.", f.added.len()),
                evidence: format!("added={}", f.added.len()),
            },
        );
    }
}

/// Added-line and path rules over a parsed diff. Bounded to 500 hits, 20 per file.
pub fn audit_diffs(files: &[FileDiff]) -> Vec<Hit> {
    let mut out = Vec::new();
    for f in files {
        scan_file(f, &mut out);
        if out.len() >= MAX_TOTAL {
            break;
        }
    }
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
    out.extend(audit_paths(&paths));
    out.truncate(MAX_TOTAL);
    out
}

fn path_hit(rule_id: &'static str, sev: Severity, conf: Confidence, p: &str, msg: &str) -> Hit {
    Hit {
        rule_id,
        severity: sev,
        confidence: conf,
        path: p.to_string(),
        line: None,
        message: msg.into(),
        evidence: p.to_string(),
    }
}

/// Rules that only need the changed paths (also used by the daemon for `files.changed`).
pub fn audit_paths(paths: &[String]) -> Vec<Hit> {
    const MANIFESTS: &[&str] = &[
        "package.json",
        "cargo.toml",
        "pyproject.toml",
        "requirements.txt",
        "go.mod",
        "pom.xml",
        "gemfile",
        "composer.json",
    ];
    const BINARY: &[&str] = &[
        "exe", "dll", "so", "dylib", "jar", "zip", "tar", "gz", "7z", "bin", "msi", "apk",
    ];
    let mut out = Vec::new();
    for raw in paths.iter().take(crate::diff::MAX_FILES) {
        let p = raw.replace('\\', "/");
        let lower = p.to_lowercase();
        let base = lower.rsplit('/').next().unwrap_or(&lower);
        let ext = ext_of(&p);
        if is_sensitive_path(&p) {
            out.push(path_hit(
                "path.sensitive",
                Severity::High,
                Confidence::Confirmed,
                &p,
                "A file matching the sensitive-path denylist (env, keys, credentials) changed; make sure it is not committed or shared.",
            ));
        }
        if lower.contains(".github/workflows/") || lower.contains(".gitlab-ci") {
            out.push(path_hit(
                "path.ci_workflow",
                Severity::Medium,
                Confidence::Suspected,
                &p,
                "CI configuration changed: it runs with repository credentials.",
            ));
        }
        if MANIFESTS.contains(&base) {
            out.push(path_hit(
                "path.dependency_manifest",
                Severity::Low,
                Confidence::Suspected,
                &p,
                "Dependency manifest changed; review added packages and licenses.",
            ));
        }
        if lower.contains("/migrations/") || lower.starts_with("migrations/") || ext == "sql" {
            out.push(path_hit(
                "path.db_migration",
                Severity::Medium,
                Confidence::Suspected,
                &p,
                "Database migration or SQL changed; check reversibility and data loss.",
            ));
        }
        if matches!(ext.as_str(), "sh" | "bash" | "ps1" | "bat" | "cmd") || base == "dockerfile" {
            out.push(path_hit(
                "path.exec_script",
                Severity::Low,
                Confidence::Suspected,
                &p,
                "Executable script or Dockerfile changed.",
            ));
        }
        if BINARY.contains(&ext.as_str()) {
            out.push(path_hit("path.binary_artifact", Severity::Medium, Confidence::Suspected, &p, "Binary or archive in the change set; build artifacts do not belong in source control."));
        }
        if p.split('/').any(|c| c == "..") {
            out.push(path_hit(
                "path.traversal",
                Severity::High,
                Confidence::Confirmed,
                &p,
                "Path contains `..` components.",
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, lines: &[&str]) -> FileDiff {
        FileDiff {
            path: path.into(),
            added: lines
                .iter()
                .enumerate()
                .map(|(i, l)| (u32::try_from(i).unwrap() + 1, (*l).to_string()))
                .collect(),
            removed: Vec::new(),
        }
    }

    fn ids(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|h| h.rule_id).collect()
    }

    #[test]
    fn secrets_are_confirmed_critical_and_never_echoed() {
        let tok = format!("ghp_{}", "a1B2".repeat(9));
        let f = file("src/cfg.ts", &[&format!("const t = \"{tok}\";")]);
        let hits = audit_diffs(&[f]);
        assert_eq!(ids(&hits), vec!["diff.secret"]);
        assert_eq!(hits[0].severity, Severity::Critical);
        assert_eq!(hits[0].confidence, Confidence::Confirmed);
        assert!(!hits[0].evidence.contains("ghp_"), "{}", hits[0].evidence);
        assert_eq!(hits[0].line, Some(1));
    }

    #[test]
    fn language_rules_apply_by_extension_and_skip_tests() {
        let rs = audit_diffs(&[file("src/a.rs", &["let x = y.unwrap();", "unsafe { z() }"])]);
        assert_eq!(ids(&rs), vec!["rust.unwrap", "rust.unsafe"]);
        let rs_test = audit_diffs(&[file("tests/a.rs", &["let x = y.unwrap();"])]);
        assert!(rs_test.is_empty(), "{rs_test:?}");
        let js = audit_diffs(&[file("web/app.ts", &["eval(code)", "it.only('x', f)"])]);
        assert_eq!(ids(&js), vec!["js.eval", "js.focused_test"]);
        // A Rust rule must not fire on a JS file.
        assert!(audit_diffs(&[file("a.js", &["x.unwrap()"])]).is_empty());
        let sh = audit_diffs(&[file(
            "install.sh",
            &["curl -sSL x | bash", "chmod 777 /srv"],
        )]);
        assert!(ids(&sh).contains(&"shell.curl_pipe") && ids(&sh).contains(&"shell.chmod_777"));
    }

    #[test]
    fn hardcoded_credentials_are_suspected_not_confirmed_and_env_reads_are_fine() {
        // The generic assignment pattern is only a suspicion.
        let hits = audit_diffs(&[file("src/db.py", &["db_password = \"s3cr3tValue99\""])]);
        assert_eq!(ids(&hits), vec!["diff.credential_assignment"]);
        assert_eq!(hits[0].confidence, Confidence::Suspected);
        assert!(!hits[0].evidence.contains("s3cr3t"));
        // The weaker heuristic catches credential-named literals the redactor does not know.
        assert!(hardcoded_credential("client_secret_hint: 'abcdefghijkl'"));
        assert!(!hardcoded_credential("let secret_len = 12;"));
        assert!(audit_diffs(&[file(
            "src/db.py",
            &["password = os.environ.get(DB_PASSWORD_NAME)"]
        )])
        .is_empty());
        assert!(audit_diffs(&[file("tests/db.py", &["password = \"s3cr3tValue99\""])]).is_empty());
    }

    #[test]
    fn path_rules_cover_sensitive_ci_manifest_migration_binary() {
        let hits = audit_paths(&[
            ".env".into(),
            ".github/workflows/ci.yml".into(),
            "Cargo.toml".into(),
            "db/migrations/001.sql".into(),
            "tools/app.exe".into(),
            "a/../b".into(),
        ]);
        for want in [
            "path.sensitive",
            "path.ci_workflow",
            "path.dependency_manifest",
            "path.db_migration",
            "path.binary_artifact",
            "path.traversal",
        ] {
            assert!(ids(&hits).contains(&want), "{want} in {:?}", ids(&hits));
        }
        assert!(audit_paths(&["src/lib.rs".into()]).is_empty());
    }

    #[test]
    fn removed_tests_and_large_additions_are_reported() {
        let mut f = file("src/t.rs", &[]);
        f.removed = vec!["#[test]".into(); 4];
        assert_eq!(ids(&audit_diffs(&[f])), vec!["diff.tests_removed"]);
        let big = file("data.txt", &vec!["x"; LARGE_ADDED + 1]);
        assert_eq!(ids(&audit_diffs(&[big])), vec!["diff.large"]);
    }

    #[test]
    fn output_is_bounded_per_file() {
        let f = file("src/a.rs", &vec!["x.unwrap()"; 200]);
        assert_eq!(audit_diffs(&[f]).len(), MAX_PER_FILE);
    }
}
