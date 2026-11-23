//! Guardian (KPL-013): the security and context-budget side of the pipeline.
//!
//! * **Preflight** (fail-closed): built-in sensitive-path / destructive-command checks plus
//!   optional user rules that can only *add* restrictions (never allow something the
//!   built-ins deny). A panic or any doubt means `deny`, never `passthrough`.
//! * **Approvals**: Guardian only returns `require_approval`; it never approves anything
//!   itself. The harness must obtain the user's answer. Nothing is remembered.
//! * **Result scan**: tool output excerpts are checked for secrets (`redact` action).
//!   The excerpt is never stored and the secret is never echoed back.
//! * **Budget stage**: [`BudgetStage`] dedupes and trims derived context to a token budget
//!   (estimated tokens, always labeled as estimates).
//!
//! Limits, stated honestly: Guardian governs only what a harness routes through it; it
//! cannot see commands the harness does not report, and string matching is not a sandbox.

use crate::decision::{Decision, Derived};
use crate::engine::{ModuleError, OutputStage};
use crate::preflight::preflight_tool_call;
use crate::redact::detect_secrets;
use crate::tokens::{estimate_tokens, ModelFamily};
use kepler_events::{Envelope, Payload, ToolCallCompleted, ToolCallRequested};

pub const MAX_RULES: usize = 256;
const MAX_RULE_CHARS: usize = 200;

/// Extra restrictions from the user's rules file. They can only tighten the built-ins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuardianRules {
    pub deny_paths: Vec<String>,
    pub approve_paths: Vec<String>,
    pub deny_commands: Vec<String>,
    pub approve_commands: Vec<String>,
}

impl GuardianRules {
    /// Parse the line format; returns the rules and one warning per rejected line.
    ///
    /// ```text
    /// # comment
    /// deny-path secrets/            (path prefix or segment, case-insensitive, / and \ equal)
    /// approve-path infra/prod/
    /// deny-command shutdown         (phrase inside the whitespace-collapsed command)
    /// approve-command kubectl apply
    /// ```
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut rules = Self::default();
        let mut warnings = Vec::new();
        let mut count = 0usize;
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let n = i + 1;
            let Some((kind, arg)) = line.split_once(char::is_whitespace) else {
                warnings.push(format!("line {n}: expected `<kind> <pattern>`"));
                continue;
            };
            let arg = arg.trim();
            if arg.is_empty() || arg.chars().count() > MAX_RULE_CHARS || arg.contains('\0') {
                warnings.push(format!("line {n}: invalid pattern"));
                continue;
            }
            count += 1;
            if count > MAX_RULES {
                warnings.push(format!("line {n}: more than {MAX_RULES} rules, ignored"));
                break;
            }
            match kind {
                "deny-path" => rules.deny_paths.push(norm_path(arg)),
                "approve-path" => rules.approve_paths.push(norm_path(arg)),
                "deny-command" => rules.deny_commands.push(collapse(arg)),
                "approve-command" => rules.approve_commands.push(collapse(arg)),
                other => warnings.push(format!("line {n}: unknown rule kind `{other}`")),
            }
        }
        (rules, warnings)
    }

    pub fn is_empty(&self) -> bool {
        self.deny_paths.is_empty()
            && self.approve_paths.is_empty()
            && self.deny_commands.is_empty()
            && self.approve_commands.is_empty()
    }
}

fn collapse(s: &str) -> String {
    s.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn norm_path(p: &str) -> String {
    p.to_lowercase().replace('\\', "/")
}

/// Prefix match on the whole path or on any directory boundary inside it.
fn path_matches(path: &str, pattern: &str) -> bool {
    let p = norm_path(path);
    let p = p.trim_start_matches("./");
    if p.starts_with(pattern) {
        return true;
    }
    let needle = format!("/{}", pattern.trim_start_matches('/'));
    p.contains(&needle)
}

/// System locations a coding agent has no business writing to without asking.
const SYSTEM_PREFIXES: &[&str] = &[
    "/etc/",
    "/boot/",
    "/sys/",
    "/proc/",
    "/dev/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "c:/windows/",
    "c:/program files",
    "c:/programdata/",
];

/// Git internals that change behaviour without showing up in a diff.
const GIT_INTERNALS: &[&str] = &[".git/hooks/", ".git/config", ".gitmodules"];

pub struct Guardian {
    rules: GuardianRules,
}

impl Default for Guardian {
    fn default() -> Self {
        Self::new(GuardianRules::default())
    }
}

impl Guardian {
    pub fn new(rules: GuardianRules) -> Self {
        Self { rules }
    }

    pub fn rules(&self) -> &GuardianRules {
        &self.rules
    }

    /// Verdict for a tool call. Deterministic; the built-in checks win over user rules.
    pub fn preflight(&self, call: &ToolCallRequested) -> Decision {
        let base = preflight_tool_call(call);
        if matches!(base, Decision::Deny { .. }) {
            return base;
        }
        let cmd = call.command.as_deref().map(collapse);
        if self.rules.deny_paths.iter().any(|r| {
            call.paths.iter().any(|p| path_matches(p, r))
                || cmd
                    .as_deref()
                    .is_some_and(|c| c.replace('\\', "/").contains(r))
        }) {
            return Decision::Deny {
                reason: "user_rule_deny_path".into(),
            };
        }
        if let Some(c) = &cmd {
            if self.rules.deny_commands.iter().any(|r| c.contains(r)) {
                return Decision::Deny {
                    reason: "user_rule_deny_command".into(),
                };
            }
        }
        if !matches!(base, Decision::Passthrough { .. }) {
            return base; // already RequireApproval
        }
        if let Some(reason) = builtin_path_approval(&call.paths) {
            return Decision::RequireApproval {
                reason: reason.into(),
            };
        }
        if self
            .rules
            .approve_paths
            .iter()
            .any(|r| call.paths.iter().any(|p| path_matches(p, r)))
        {
            return Decision::RequireApproval {
                reason: "user_rule_approve_path".into(),
            };
        }
        if let Some(c) = &cmd {
            if self.rules.approve_commands.iter().any(|r| c.contains(r)) {
                return Decision::RequireApproval {
                    reason: "user_rule_approve_command".into(),
                };
            }
        }
        base
    }

    /// Verdict for a completed tool call: `Redact` when the output excerpt holds secrets.
    pub fn scan_result(&self, done: &ToolCallCompleted) -> Decision {
        let Some(text) = done.output_excerpt.as_deref() else {
            return Decision::passthrough("observe_only");
        };
        let kinds = detect_secrets(text);
        if kinds.is_empty() {
            Decision::passthrough("result_clean")
        } else {
            Decision::Redact {
                reason: "secret_in_tool_result".into(),
                kinds: kinds.into_iter().map(String::from).collect(),
            }
        }
    }
}

fn builtin_path_approval(paths: &[String]) -> Option<&'static str> {
    for p in paths {
        let n = norm_path(p);
        let n = n.trim_start_matches("./");
        if SYSTEM_PREFIXES.iter().any(|s| n.starts_with(s)) {
            return Some("system_path");
        }
        if GIT_INTERNALS
            .iter()
            .any(|g| n == *g || n.contains(&format!("/{g}")) || n.starts_with(g))
        {
            return Some("git_internals");
        }
    }
    None
}

/// Default Guardian context budget (estimated tokens) for all derived items of one reply.
pub const DEFAULT_CONTEXT_BUDGET_TOKENS: u32 = 1200;

/// Output stage: dedupe near-identical derived items and keep the earliest items that fit the
/// token budget (modules run in pipeline order, so earlier = Forge, then Memory). A
/// `context.requested` event's own `budget_tokens` caps it further. Items are kept or dropped
/// whole; text is never rewritten or truncated.
pub struct BudgetStage {
    pub budget_tokens: u32,
    pub family: ModelFamily,
}

impl Default for BudgetStage {
    fn default() -> Self {
        Self {
            budget_tokens: DEFAULT_CONTEXT_BUDGET_TOKENS,
            family: ModelFamily::Generic,
        }
    }
}

fn word_set(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn similarity(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    inter / ((a.len() + b.len()) as f64 - inter)
}

impl OutputStage for BudgetStage {
    fn name(&self) -> &'static str {
        "guardian"
    }

    fn refine(&self, event: &Envelope, derived: Vec<Derived>) -> Result<Vec<Derived>, ModuleError> {
        let mut budget = self.budget_tokens;
        if let Payload::ContextRequested(req) = &event.payload {
            budget = budget.min(req.budget_tokens);
        }
        let mut kept: Vec<Derived> = Vec::new();
        let mut sets: Vec<std::collections::HashSet<String>> = Vec::new();
        let mut used = 0u32;
        for d in derived {
            let set = word_set(&d.text);
            if sets.iter().any(|s| similarity(s, &set) >= 0.9) {
                continue;
            }
            let cost = estimate_tokens(&d.text, self.family).tokens;
            if used.saturating_add(cost) > budget {
                continue;
            }
            used += cost;
            sets.push(set);
            kept.push(d);
        }
        Ok(kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(cmd: Option<&str>, paths: &[&str]) -> ToolCallRequested {
        ToolCallRequested {
            tool: "bash".into(),
            command: cmd.map(Into::into),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            destructive: false,
        }
    }

    fn done(excerpt: Option<&str>) -> ToolCallCompleted {
        ToolCallCompleted {
            tool: "bash".into(),
            ok: true,
            output_bytes: 10,
            duration_ms: 1,
            output_excerpt: excerpt.map(Into::into),
        }
    }

    fn ev(kind: &str, payload: &str) -> Envelope {
        let json = format!(
            r#"{{"version":1,"id":"e1","type":"{kind}","timestamp":"2026-01-02T03:04:05Z","harness":"pi","project_id":"p","session_id":"s","correlation_id":"c","payload":{payload}}}"#
        );
        Envelope::parse(json.as_bytes()).unwrap()
    }

    #[test]
    fn rules_parse_with_warnings() {
        let (r, w) = GuardianRules::parse(
            "# c\n\ndeny-path Secrets\\Prod/\napprove-command  kubectl   apply\nbogus x\ndeny-command\nallow-path /\n",
        );
        assert_eq!(r.deny_paths, vec!["secrets/prod/"]);
        assert_eq!(r.approve_commands, vec!["kubectl apply"]);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(GuardianRules::parse("").0.is_empty());
        let many: String = (0..300).map(|i| format!("deny-command c{i}\n")).collect();
        let (r, w) = GuardianRules::parse(&many);
        assert_eq!(r.deny_commands.len(), MAX_RULES);
        assert!(!w.is_empty());
    }

    #[test]
    fn user_rules_only_tighten() {
        let (rules, _) = GuardianRules::parse(
            "deny-path infra/prod\napprove-path migrations/\ndeny-command terraform destroy\napprove-command kubectl apply\n",
        );
        let g = Guardian::new(rules);
        assert!(matches!(
            g.preflight(&call(None, &["infra/prod/main.tf"])),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            g.preflight(&call(None, &["a/b/infra/prod/x"])),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            g.preflight(&call(None, &["migrations/001.sql"])),
            Decision::RequireApproval { .. }
        ));
        assert!(matches!(
            g.preflight(&call(Some("terraform  destroy -auto-approve"), &[])),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            g.preflight(&call(Some("kubectl apply -f x.yaml"), &[])),
            Decision::RequireApproval { .. }
        ));
        assert!(g
            .preflight(&call(Some("cargo test"), &["src/lib.rs"]))
            .is_passthrough());
        // built-ins still win: a user rule cannot downgrade a deny
        assert!(matches!(
            g.preflight(&call(Some("cat .env"), &[])),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            g.preflight(&call(Some("rm -rf /"), &[])),
            Decision::RequireApproval { .. }
        ));
    }

    #[test]
    fn builtin_system_and_git_internals_need_approval() {
        let g = Guardian::default();
        for p in [
            "/etc/hosts",
            "C:\\Windows\\System32\\drivers\\etc\\hosts",
            "/usr/bin/env",
            ".git/hooks/pre-commit",
            "sub/.git/config",
            ".gitmodules",
        ] {
            assert!(
                matches!(
                    g.preflight(&call(None, &[p])),
                    Decision::RequireApproval { .. }
                ),
                "{p}"
            );
        }
        for p in ["src/etc/hosts.rs", "docs/git.md", "my.gitignore"] {
            assert!(g.preflight(&call(None, &[p])).is_passthrough(), "{p}");
        }
    }

    #[test]
    fn extended_destructive_commands() {
        let g = Guardian::default();
        for c in [
            "npm publish",
            "cargo publish --token x",
            "terraform destroy",
            "kubectl delete ns prod",
            "docker system prune -af",
            "docker push img",
            "sudo shutdown -h now",
            "Format-Volume -DriveLetter D",
            "reg delete HKLM\\x",
        ] {
            assert!(
                matches!(
                    g.preflight(&call(Some(c), &[])),
                    Decision::RequireApproval { .. }
                ),
                "{c}"
            );
        }
    }

    #[test]
    fn result_scan_flags_secrets_and_never_echoes_them() {
        let g = Guardian::default();
        assert!(g.scan_result(&done(None)).is_passthrough());
        assert!(g
            .scan_result(&done(Some("test result: ok. 12 passed")))
            .is_passthrough());
        match g.scan_result(&done(Some(
            "DATABASE_URL=postgres://u:pw@h/db\nAWS key AKIAIOSFODNN7EXAMPLE",
        ))) {
            Decision::Redact { reason, kinds } => {
                assert_eq!(reason, "secret_in_tool_result");
                assert!(kinds.contains(&"aws_access_key".to_string()));
                assert!(kinds.iter().all(|k| !k.contains("AKIA")));
            }
            d => panic!("{d:?}"),
        }
    }

    #[test]
    fn budget_stage_dedupes_and_trims_whole_items() {
        let stage = BudgetStage {
            budget_tokens: 40,
            family: ModelFamily::Generic,
        };
        let mk = |s: &str, t: &str| Derived::new(s, t, "inferred");
        let out = stage
            .refine(
                &ev("input.received", r#"{"text":"x"}"#),
                vec![
                    mk("forge", "task looks like a bug fix and has open questions"),
                    mk(
                        "memory",
                        "Task looks like a bug fix and has open questions!",
                    ),
                    mk("memory", &"word ".repeat(100)),
                    mk("memory", "short note"),
                ],
            )
            .unwrap();
        let texts: Vec<&str> = out.iter().map(|d| d.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "task looks like a bug fix and has open questions",
                "short note"
            ]
        );
    }

    #[test]
    fn context_request_budget_caps_the_stage() {
        let stage = BudgetStage::default();
        let big = Derived::new("memory", "alpha ".repeat(60), "recalled");
        let small = ev("context.requested", r#"{"budget_tokens":10}"#);
        assert!(stage.refine(&small, vec![big.clone()]).unwrap().is_empty());
        let large = ev("context.requested", r#"{"budget_tokens":1000}"#);
        assert_eq!(stage.refine(&large, vec![big]).unwrap().len(), 1);
    }
}
