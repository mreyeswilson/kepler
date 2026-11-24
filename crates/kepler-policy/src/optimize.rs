//! Guardian token optimization decisions (ADR-0027). Pure functions: the adapter does the I/O
//! (file size, line count) and applies the result. Everything is advisory or a *bound on an
//! unbounded request*; an input the model already bounded is never touched.

use crate::tokens::{estimate_tokens, ModelFamily};

pub const DEFAULT_READ_MAX_LINES: u32 = 800;
/// A tool result above this many estimated tokens earns a one-line note to the model.
pub const DEFAULT_OUTPUT_WARN_TOKENS: u32 = 6000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGuardConfig {
    /// `KEPLER_GUARDIAN_TOKENS=off` disables every optimization below.
    pub enabled: bool,
    /// `KEPLER_GUARDIAN_READ_MAX_LINES` (clamped 50..=20000).
    pub read_max_lines: u32,
    /// `KEPLER_GUARDIAN_OUTPUT_WARN_TOKENS` (clamped 500..=200000).
    pub output_warn_tokens: u32,
}

impl Default for TokenGuardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            read_max_lines: DEFAULT_READ_MAX_LINES,
            output_warn_tokens: DEFAULT_OUTPUT_WARN_TOKENS,
        }
    }
}

impl TokenGuardConfig {
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(v) = std::env::var("KEPLER_GUARDIAN_TOKENS") {
            c.enabled = !matches!(
                v.trim().to_lowercase().as_str(),
                "off" | "0" | "false" | "no"
            );
        }
        let num = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<u32>().ok())
        };
        if let Some(n) = num("KEPLER_GUARDIAN_READ_MAX_LINES") {
            c.read_max_lines = n.clamp(50, 20_000);
        }
        if let Some(n) = num("KEPLER_GUARDIAN_OUTPUT_WARN_TOKENS") {
            c.output_warn_tokens = n.clamp(500, 200_000);
        }
        c
    }
}

/// What to do with a `Read` that has no `offset`/`limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPlan {
    /// Leave the call exactly as the model made it.
    Keep,
    /// Add `limit` (from line 1) because the file has `total_lines`.
    Truncate { limit: u32, total_lines: u64 },
}

/// Only a read with neither `offset` nor `limit` (the model did not bound it) and a file longer
/// than `max_lines` is truncated.
pub fn plan_read(
    has_offset: bool,
    has_limit: bool,
    total_lines: u64,
    max_lines: u32,
) -> ReadPlan {
    if has_offset || has_limit || total_lines <= u64::from(max_lines) {
        ReadPlan::Keep
    } else {
        ReadPlan::Truncate {
            limit: max_lines,
            total_lines,
        }
    }
}

/// Estimated tokens not sent when only the first `limit` of `total_lines` lines are read
/// (proportional to the file size; an estimate, never a measurement).
pub fn estimate_read_savings(size_bytes: u64, total_lines: u64, limit: u32) -> u32 {
    if total_lines == 0 || total_lines <= u64::from(limit) {
        return 0;
    }
    let omitted = (total_lines - u64::from(limit)) as f64 / total_lines as f64;
    let total = (size_bytes as f64 / 4.0).ceil();
    (total * omitted).min(f64::from(u32::MAX)) as u32
}

/// Note the model sees next to a truncated read.
pub fn truncation_note(limit: u32, total_lines: u64) -> String {
    format!(
        "Kepler Guardian: the file has {total_lines} lines; this read was limited to the first {limit} (archivo truncado por Kepler para ahorrar contexto). If you need more, call Read again with offset/limit for the range you need. Set KEPLER_GUARDIAN_TOKENS=off to disable."
    )
}

/// Note for a repeated unchanged read of the same file in one session.
pub fn repeat_read_note(count: u32) -> String {
    format!(
        "Kepler Guardian: this file was already read {count} time(s) earlier in this session and has not changed since; prefer reusing what you already have or read only the lines you need (offset/limit)."
    )
}

/// Note for a very large tool result (estimated tokens given).
pub fn big_output_note(tool: &str, est_tokens: u32) -> String {
    let tool: String = tool
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(40)
        .collect();
    format!(
        "Kepler Guardian: the {tool} result was large (~{est_tokens} estimated tokens). Avoid pasting or re-reading it in full; narrow the next call (filters, ranges, head/tail, limits)."
    )
}

/// Estimated tokens of a tool result text (generic family; an estimate).
pub fn estimate_output_tokens(text: &str) -> u32 {
    estimate_tokens(text, ModelFamily::Generic).tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unbounded_big_reads_are_truncated() {
        assert_eq!(plan_read(false, false, 100, 800), ReadPlan::Keep);
        assert_eq!(plan_read(false, false, 800, 800), ReadPlan::Keep);
        assert_eq!(
            plan_read(false, false, 5000, 800),
            ReadPlan::Truncate {
                limit: 800,
                total_lines: 5000
            }
        );
        // The model already bounded it: never touched.
        assert_eq!(plan_read(true, false, 5000, 800), ReadPlan::Keep);
        assert_eq!(plan_read(false, true, 5000, 800), ReadPlan::Keep);
    }

    #[test]
    fn savings_are_proportional_and_zero_when_nothing_is_omitted() {
        assert_eq!(estimate_read_savings(40_000, 1000, 1000), 0);
        let s = estimate_read_savings(40_000, 2000, 500);
        assert_eq!(s, 7500); // 10000 tokens * 0.75
        assert_eq!(estimate_read_savings(0, 0, 10), 0);
    }

    #[test]
    fn notes_are_bounded_and_clean() {
        assert!(truncation_note(800, 5000).contains("5000"));
        assert!(big_output_note("Ba sh\n<x>", 9000).contains("Bashx"));
        assert!(repeat_read_note(2).contains('2'));
        assert_eq!(
            TokenGuardConfig::default().read_max_lines,
            DEFAULT_READ_MAX_LINES
        );
    }
}
