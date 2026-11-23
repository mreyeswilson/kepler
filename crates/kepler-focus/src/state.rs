//! Per-session supervision state and the rules. Pure (no I/O); time comes from `Instant`.

use crate::config::FocusConfig;
use kepler_events::{Envelope, Outcome, Payload};
use kepler_policy::{Advisory, AdvisoryLevel};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::time::Instant;

const SOURCE: &str = "focus";
const WINDOW: usize = 16;
const MAX_MENTIONS: usize = 32;
const MAX_TRACKED_FILES: usize = 256;
const MAX_FAIL_KEYS: usize = 32;

const MANIFESTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "cargo.toml",
    "cargo.lock",
    "requirements.txt",
    "pyproject.toml",
    "poetry.lock",
    "go.mod",
    "go.sum",
    "gemfile",
    "gemfile.lock",
    "composer.json",
    "pom.xml",
    "build.gradle",
];
const DEP_WORDS: &[&str] = &[
    "depend", "install", "upgrade", "bump", "package", "crate", "library", "npm", "cargo", "pip",
    "version", "lockfile",
];
const VERIFY_WORDS: &[&str] = &[
    "test", "check", "build", "lint", "clippy", "pytest", "jest", "vitest", "tsc", "compile",
    "mypy", "verify",
];

fn hash_of<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn norm(p: &str) -> String {
    let s = p.replace('\\', "/").to_lowercase();
    s.trim_start_matches("./").to_string()
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn parent(p: &str) -> &str {
    p.rsplit_once('/').map_or("", |(d, _)| d)
}

/// File-looking tokens the user typed (`src/app.rs`, `index.ts`). Names only, never the text.
pub(crate) fn mentioned_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let is_sep = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '`' | ',' | '(' | ')' | '[' | ']' | '<' | '>'
            )
    };
    for tok in text.split(is_sep) {
        let t = tok.trim_end_matches(['.', ':', ';', '!', '?']);
        if t.is_empty() || t.len() > 200 || t.contains("://") {
            continue;
        }
        let file_like = basename(t).rsplit_once('.').is_some_and(|(_, e)| {
            e.len() >= 2
                && e.len() <= 8
                && e.chars().all(|c| c.is_ascii_alphanumeric())
                && e.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        });
        if file_like || (t.contains('/') && t.len() > 2 && !t.starts_with('/')) {
            let n = norm(t);
            if !out.contains(&n) {
                out.push(n);
            }
            if out.len() >= MAX_MENTIONS {
                break;
            }
        }
    }
    out
}

fn related(changed: &str, mentions: &[String]) -> bool {
    let c = norm(changed);
    mentions.iter().any(|m| {
        c == *m
            || c.ends_with(&format!("/{m}"))
            || m.ends_with(&format!("/{c}"))
            || basename(&c) == basename(m)
            || (!parent(m).is_empty() && parent(&c) == parent(m))
            || (!m.contains('.') && c.contains(m.as_str()))
    })
}

fn looks_like_verification(tool: &str, command: Option<&str>) -> bool {
    let hay = format!("{} {}", tool, command.unwrap_or("")).to_lowercase();
    VERIFY_WORDS.iter().any(|w| hay.contains(w))
}

#[derive(Default)]
pub(crate) struct SessionState {
    pub(crate) last_used: u64,
    started: Option<Instant>,
    mentions: Vec<String>,
    mentions_deps: bool,
    seq: u64,
    tool_calls: u32,
    calls_since_change: u32,
    fails_in_row: u32,
    /// (call fingerprint, change epoch) of recent requests.
    recent: VecDeque<(u64, u32)>,
    epoch: u32,
    change_sets: VecDeque<u64>,
    last_request: Option<(u64, bool)>,
    fail_counts: HashMap<(u64, u32), u32>,
    changed_files: HashSet<String>,
    unrelated: HashSet<String>,
    last_change_seq: u64,
    last_verify_seq: u64,
    calls_after_verify: u32,
    emitted: HashMap<&'static str, u64>,
    advisories: u32,
}

impl SessionState {
    /// A new user input starts a new task: counters and windows restart. Only file names
    /// the user mentioned are kept, never the text.
    pub(crate) fn begin_task(&mut self, text: &str) {
        let used = self.last_used;
        let emitted = std::mem::take(&mut self.emitted);
        let advisories = self.advisories;
        *self = Self::default();
        self.last_used = used;
        self.emitted = emitted;
        self.advisories = advisories;
        self.started = Some(Instant::now());
        self.mentions = mentioned_paths(text);
        let lower = text.to_lowercase();
        self.mentions_deps = DEP_WORDS.iter().any(|w| lower.contains(w));
    }

    fn emit(
        &mut self,
        cfg: &FocusConfig,
        out: &mut Vec<Advisory>,
        code: &'static str,
        level: AdvisoryLevel,
        message: String,
    ) {
        if self.advisories >= cfg.max_advisories_per_session {
            return;
        }
        if let Some(at) = self.emitted.get(code) {
            if self.seq.saturating_sub(*at) < cfg.cooldown_events {
                return;
            }
        }
        self.emitted.insert(code, self.seq);
        self.advisories += 1;
        out.push(Advisory::new(SOURCE, level, code, message));
    }

    pub(crate) fn observe(&mut self, cfg: &FocusConfig, event: &Envelope) -> Vec<Advisory> {
        let mut out = Vec::new();
        self.seq += 1;
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        match &event.payload {
            Payload::ToolCallRequested(c) => {
                self.on_request(cfg, &mut out, &c.tool, c.command.as_deref(), &c.paths);
            }
            Payload::ToolCallCompleted(c) => self.on_completed(cfg, &mut out, c.ok),
            Payload::FilesChanged(f) => self.on_files(cfg, &mut out, &f.paths),
            Payload::AgentSettled(s) if s.outcome == Outcome::Ok => self.on_settled(cfg, &mut out),
            _ => {}
        }
        out
    }

    fn on_request(
        &mut self,
        cfg: &FocusConfig,
        out: &mut Vec<Advisory>,
        tool: &str,
        command: Option<&str>,
        paths: &[String],
    ) {
        self.tool_calls = self.tool_calls.saturating_add(1);
        self.calls_since_change = self.calls_since_change.saturating_add(1);
        if self.last_verify_seq > self.last_change_seq {
            self.calls_after_verify = self.calls_after_verify.saturating_add(1);
        }
        let fp = hash_of(&(tool, command, paths));
        self.last_request = Some((fp, looks_like_verification(tool, command)));
        self.recent.push_back((fp, self.epoch));
        if self.recent.len() > WINDOW {
            self.recent.pop_front();
        }
        let same = self
            .recent
            .iter()
            .filter(|(f, e)| *f == fp && *e == self.epoch)
            .count();
        if u32::try_from(same).unwrap_or(u32::MAX) >= cfg.repeat_threshold {
            self.emit(
                cfg,
                out,
                "loop.repeated_call",
                AdvisoryLevel::Warn,
                format!(
                    "The same `{tool}` call was issued {same} times with no file change in between. Change the approach or explain what is expected to differ."
                ),
            );
        }
        self.check_budget(cfg, out);
        if !self.changed_files.is_empty()
            && self.last_verify_seq > self.last_change_seq
            && self.calls_after_verify >= cfg.post_verify_calls
        {
            let n = self.calls_after_verify;
            self.emit(
                cfg,
                out,
                "completion.recommend_stop",
                AdvisoryLevel::RecommendStop,
                format!(
                    "A verification passed after the last change and {n} further calls changed nothing. If the request is satisfied, report and stop; avoid unrequested polishing."
                ),
            );
        }
    }

    fn check_budget(&mut self, cfg: &FocusConfig, out: &mut Vec<Advisory>) {
        let elapsed = self.started.map(|s| s.elapsed()).unwrap_or_default();
        let over_calls = self.tool_calls >= cfg.soft_tool_calls;
        let over_time = elapsed >= cfg.soft_time;
        if !(over_calls || over_time) {
            return;
        }
        let what = if over_calls {
            format!("{} tool calls", self.tool_calls)
        } else {
            format!("{} s elapsed", elapsed.as_secs())
        };
        if self.calls_since_change >= cfg.stall_calls {
            let n = self.calls_since_change;
            self.emit(
                cfg,
                out,
                "budget.stalled",
                AdvisoryLevel::RecommendStop,
                format!(
                    "Budget exceeded ({what}) and {n} calls without any file change. Consider reporting progress and stopping; Kepler cancels nothing."
                ),
            );
        } else {
            self.emit(
                cfg,
                out,
                "budget.exceeded",
                AdvisoryLevel::Warn,
                format!(
                    "Soft budget reached ({what}) but files are still changing; reassess scope. Productive work is not interrupted."
                ),
            );
        }
    }

    fn on_completed(&mut self, cfg: &FocusConfig, out: &mut Vec<Advisory>, ok: bool) {
        let Some((fp, verifying)) = self.last_request else {
            return;
        };
        if ok {
            self.fails_in_row = 0;
            if verifying {
                self.last_verify_seq = self.seq;
                self.calls_after_verify = 0;
            }
            return;
        }
        self.fails_in_row += 1;
        if self.fail_counts.len() >= MAX_FAIL_KEYS {
            self.fail_counts.clear();
        }
        let n = {
            let c = self.fail_counts.entry((fp, self.epoch)).or_insert(0);
            *c += 1;
            *c
        };
        if n >= cfg.failure_threshold {
            self.emit(
                cfg,
                out,
                "loop.repeated_failure",
                AdvisoryLevel::Warn,
                format!(
                    "The same call failed {n} times with no change in between. A retry is only useful if something changed."
                ),
            );
        } else if self.fails_in_row >= cfg.failure_streak {
            let s = self.fails_in_row;
            self.emit(
                cfg,
                out,
                "loop.failure_streak",
                AdvisoryLevel::Warn,
                format!(
                    "{s} consecutive tool calls failed. Reconsider the approach before continuing."
                ),
            );
        }
    }

    fn on_files(&mut self, cfg: &FocusConfig, out: &mut Vec<Advisory>, paths: &[String]) {
        self.epoch = self.epoch.wrapping_add(1);
        self.calls_since_change = 0;
        self.calls_after_verify = 0;
        self.last_change_seq = self.seq;
        // An edit between failures is progress, not a streak (a retry after a change is legitimate).
        self.fails_in_row = 0;
        let mut sorted: Vec<String> = paths.iter().map(|p| norm(p)).collect();
        sorted.sort();
        sorted.dedup();
        self.change_sets.push_back(hash_of(&sorted));
        if self.change_sets.len() > 8 {
            self.change_sets.pop_front();
        }
        let cs: Vec<u64> = self.change_sets.iter().copied().collect();
        if let [.., a, b, c, d] = cs.as_slice() {
            if a == c && b == d && a != b {
                self.emit(
                    cfg,
                    out,
                    "loop.oscillating_changes",
                    AdvisoryLevel::Warn,
                    "Edits alternate between the same two change sets (A, B, A, B). Likely oscillation; settle on one approach.".into(),
                );
            }
        }
        for p in &sorted {
            if self.changed_files.len() < MAX_TRACKED_FILES {
                self.changed_files.insert(p.clone());
            }
            if !self.mentions.is_empty()
                && !related(p, &self.mentions)
                && self.unrelated.len() < MAX_TRACKED_FILES
            {
                self.unrelated.insert(p.clone());
            }
        }
        if !self.mentions_deps {
            if let Some(m) = sorted.iter().find(|p| MANIFESTS.contains(&basename(p))) {
                let name = basename(m).to_string();
                self.emit(
                    cfg,
                    out,
                    "minimal.dependency_change",
                    AdvisoryLevel::Warn,
                    format!(
                        "`{name}` changed but the request did not mention dependencies. Confirm this is necessary and say why."
                    ),
                );
            }
        }
        if self.unrelated.len() >= cfg.unrelated_files {
            let n = self.unrelated.len();
            self.emit(
                cfg,
                out,
                "scope.unrelated_changes",
                AdvisoryLevel::Warn,
                format!(
                    "{n} changed files are unrelated to the files named in the request. Justify them or revert; a justified exception is fine."
                ),
            );
        }
        if self.changed_files.len() > cfg.max_changed_files {
            let n = self.changed_files.len();
            self.emit(
                cfg,
                out,
                "minimal.large_change",
                AdvisoryLevel::Warn,
                format!("{n} files changed in this task. Check that the diff is proportionate to the request."),
            );
        }
    }

    fn on_settled(&mut self, cfg: &FocusConfig, out: &mut Vec<Advisory>) {
        if self.changed_files.is_empty() {
            return;
        }
        let n = self.changed_files.len();
        if self.last_verify_seq > self.last_change_seq {
            self.emit(
                cfg,
                out,
                "completion.verified",
                AdvisoryLevel::Info,
                format!(
                    "Observed: {n} files changed and a passing test/build/check after the last change. Acceptance criteria were not evaluated by Kepler."
                ),
            );
        } else {
            self.emit(
                cfg,
                out,
                "evidence.unverified_completion",
                AdvisoryLevel::Warn,
                format!(
                    "Observed: {n} files changed; no passing test/build/check was observed after the last change (unknown, not assumed). Claims of working code are unsupported until verified."
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_extract_file_names_only() {
        let m = mentioned_paths(
            "Fix `src/app.rs` and README.md, e.g. not this. See http://x.io/a.html v1.2",
        );
        assert_eq!(m, vec!["src/app.rs", "readme.md"]);
    }

    #[test]
    fn related_matches_name_suffix_and_directory() {
        let m = vec!["src/app.rs".to_string()];
        assert!(related("src/app.rs", &m));
        assert!(related("C:/p/src/app.rs", &m));
        assert!(related("src/other.rs", &m));
        assert!(!related("docs/guide.md", &m));
    }
}
