use std::time::Duration;

/// Thresholds are hypotheses to be calibrated against the baseline benchmark (KPL-032),
/// not universal limits; every one can be overridden.
#[derive(Debug, Clone)]
pub struct FocusConfig {
    /// Identical call with no file change in between, this many times in the window.
    pub repeat_threshold: u32,
    /// Same failing call (no change in between) this many times.
    pub failure_threshold: u32,
    /// Consecutive failed tool calls with no file change in between.
    pub failure_streak: u32,
    /// Warn when the tool-call count since the last user input reaches this.
    pub soft_tool_calls: u32,
    /// Warn when wall time since the last user input reaches this.
    pub soft_time: Duration,
    /// Over budget and this many calls without a file change: recommend stopping.
    pub stall_calls: u32,
    /// After a passing verification, this many further calls without change: recommend stopping.
    pub post_verify_calls: u32,
    /// More changed files than this is flagged as a large change.
    pub max_changed_files: usize,
    /// This many changed files unrelated to the files the user named.
    pub unrelated_files: usize,
    /// An advisory code repeats only after this many events.
    pub cooldown_events: u64,
    /// Hard cap of advisories per session (bounded noise).
    pub max_advisories_per_session: u32,
    /// Live sessions tracked; the least recently used is evicted.
    pub max_sessions: usize,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self {
            repeat_threshold: 4,
            failure_threshold: 3,
            failure_streak: 5,
            soft_tool_calls: 150,
            soft_time: Duration::from_secs(30 * 60),
            stall_calls: 30,
            post_verify_calls: 8,
            max_changed_files: 15,
            unrelated_files: 3,
            cooldown_events: 12,
            max_advisories_per_session: 40,
            max_sessions: 64,
        }
    }
}

impl FocusConfig {
    /// Defaults overridden by `KEPLER_FOCUS_SOFT_TOOL_CALLS` and `KEPLER_FOCUS_SOFT_SECONDS`
    /// (invalid or zero values are ignored).
    pub fn from_env() -> Self {
        let mut c = Self::default();
        let num = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|n| *n > 0)
        };
        if let Some(n) = num("KEPLER_FOCUS_SOFT_TOOL_CALLS") {
            c.soft_tool_calls = u32::try_from(n).unwrap_or(u32::MAX);
        }
        if let Some(n) = num("KEPLER_FOCUS_SOFT_SECONDS") {
            c.soft_time = Duration::from_secs(n);
        }
        c
    }
}
