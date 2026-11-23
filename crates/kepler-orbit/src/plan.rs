//! Plan file (JSON), validation, cycle detection and destructive-action classification.

use kepler_events::ToolCallRequested;
use kepler_policy::redact::detect_secrets;
use kepler_policy::{Decision, Guardian};
use kepler_storage::{NewTask, MAX_TASKS_PER_RUN};
use serde::Deserialize;
use std::collections::HashMap;

pub const MAX_DEPS_PER_TASK: usize = 16;
pub const MAX_ARGV: usize = 32;
pub const MAX_ARG_BYTES: usize = 1024;
pub const MAX_PLAN_BYTES: usize = 256 * 1024;
pub const MAX_PARALLEL: u32 = 4;
pub const DEFAULT_PARALLEL: u32 = 2;
pub const DEFAULT_TIMEOUT_SECS: u32 = 600;
pub const MAX_TIMEOUT_SECS: u32 = 3600;
pub const MAX_RETRIES: u32 = 3;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanFile {
    pub name: String,
    #[serde(default)]
    pub max_parallel: Option<u32>,
    pub tasks: Vec<PlanTask>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanTask {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// argv, never a shell string: `["cargo", "test"]`.
    pub command: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Force an approval checkpoint even for a harmless-looking command.
    #[serde(default)]
    pub require_approval: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlanError {
    Parse(String),
    TooLarge,
    Empty,
    TooManyTasks,
    BadName,
    BadKey(String),
    Duplicate(String),
    UnknownDependency { task: String, dep: String },
    TooManyDependencies(String),
    SelfDependency(String),
    Cycle(Vec<String>),
    BadCommand { task: String, why: &'static str },
    SecretInCommand(String),
    Denied { task: String, reason: String },
    BadLimit(&'static str),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "plan is not valid: {e}"),
            Self::TooLarge => write!(f, "plan file exceeds {MAX_PLAN_BYTES} bytes"),
            Self::Empty => write!(f, "plan has no tasks"),
            Self::TooManyTasks => write!(f, "plan has more than {MAX_TASKS_PER_RUN} tasks"),
            Self::BadName => write!(f, "plan name must be 1 to 80 printable characters"),
            Self::BadKey(k) => write!(f, "task id {k:?} must match [A-Za-z0-9._-]{{1,64}}"),
            Self::Duplicate(k) => write!(f, "duplicate task id {k:?}"),
            Self::UnknownDependency { task, dep } => {
                write!(f, "task {task:?} depends on unknown task {dep:?}")
            }
            Self::TooManyDependencies(t) => {
                write!(f, "task {t:?} has more than {MAX_DEPS_PER_TASK} dependencies")
            }
            Self::SelfDependency(t) => write!(f, "task {t:?} depends on itself"),
            Self::Cycle(path) => write!(f, "dependency cycle: {}", path.join(" -> ")),
            Self::BadCommand { task, why } => write!(f, "task {task:?}: {why}"),
            Self::SecretInCommand(t) => write!(
                f,
                "task {t:?}: the command contains a secret-shaped value; pass secrets through the environment"
            ),
            Self::Denied { task, reason } => {
                write!(f, "task {task:?} is refused by the Guardian ({reason})")
            }
            Self::BadLimit(w) => write!(f, "{w}"),
        }
    }
}

impl std::error::Error for PlanError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidPlan {
    pub name: String,
    pub max_parallel: u32,
    /// In file order (also a valid topological order is available via [`ValidPlan::order`]).
    pub tasks: Vec<NewTask>,
}

impl ValidPlan {
    /// A topological order (dependencies first), stable with respect to file order.
    pub fn order(&self) -> Vec<String> {
        topo_order(&self.tasks).unwrap_or_default()
    }
}

fn key_ok(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 64
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Kahn's algorithm, picking the earliest task in file order each round.
fn topo_order(tasks: &[NewTask]) -> Option<Vec<String>> {
    let mut done: Vec<&str> = Vec::new();
    let mut left: Vec<&NewTask> = tasks.iter().collect();
    while !left.is_empty() {
        let pos = left
            .iter()
            .position(|t| t.deps.iter().all(|d| done.contains(&d.as_str())))?;
        let t = left.remove(pos);
        done.push(&t.key);
    }
    Some(done.into_iter().map(String::from).collect())
}

/// Find one cycle by depth-first search (only called when no topological order exists).
fn find_cycle(tasks: &[NewTask]) -> Vec<String> {
    let idx: HashMap<&str, &NewTask> = tasks.iter().map(|t| (t.key.as_str(), t)).collect();
    // 0 = unvisited, 1 = on the stack, 2 = finished
    let mut state: HashMap<&str, u8> = HashMap::new();
    fn dfs<'a>(
        k: &'a str,
        idx: &HashMap<&'a str, &'a NewTask>,
        state: &mut HashMap<&'a str, u8>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        state.insert(k, 1);
        stack.push(k);
        if let Some(t) = idx.get(k) {
            for d in &t.deps {
                match state.get(d.as_str()).copied().unwrap_or(0) {
                    1 => {
                        let start = stack.iter().position(|s| *s == d.as_str()).unwrap_or(0);
                        let mut cyc: Vec<String> =
                            stack[start..].iter().map(|s| (*s).to_string()).collect();
                        cyc.push(d.clone());
                        return Some(cyc);
                    }
                    0 => {
                        if let Some(c) = dfs(d.as_str(), idx, state, stack) {
                            return Some(c);
                        }
                    }
                    _ => {}
                }
            }
        }
        stack.pop();
        state.insert(k, 2);
        None
    }
    for t in tasks {
        if state.get(t.key.as_str()).copied().unwrap_or(0) == 0 {
            let mut stack = Vec::new();
            if let Some(c) = dfs(&t.key, &idx, &mut state, &mut stack) {
                return c;
            }
        }
    }
    Vec::new()
}

/// Parse a plan from JSON text (size-limited).
pub fn parse_plan(text: &str) -> Result<PlanFile, PlanError> {
    if text.len() > MAX_PLAN_BYTES {
        return Err(PlanError::TooLarge);
    }
    serde_json::from_str(text).map_err(|e| PlanError::Parse(e.to_string()))
}

/// Validate structure, limits and safety. Destructive commands become approval checkpoints;
/// commands the Guardian denies outright make the whole plan invalid (fail closed).
pub fn validate(plan: &PlanFile, guardian: &Guardian) -> Result<ValidPlan, PlanError> {
    if plan.tasks.is_empty() {
        return Err(PlanError::Empty);
    }
    if plan.tasks.len() > MAX_TASKS_PER_RUN {
        return Err(PlanError::TooManyTasks);
    }
    if plan.name.is_empty()
        || plan.name.chars().count() > 80
        || plan.name.chars().any(char::is_control)
    {
        return Err(PlanError::BadName);
    }
    let max_parallel = plan.max_parallel.unwrap_or(DEFAULT_PARALLEL);
    if !(1..=MAX_PARALLEL).contains(&max_parallel) {
        return Err(PlanError::BadLimit("max_parallel must be 1 to 4"));
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut tasks = Vec::new();
    for t in &plan.tasks {
        if !key_ok(&t.id) {
            return Err(PlanError::BadKey(t.id.clone()));
        }
        if seen.contains(&t.id.as_str()) {
            return Err(PlanError::Duplicate(t.id.clone()));
        }
        seen.push(&t.id);
        if t.command.is_empty() || t.command.len() > MAX_ARGV {
            return Err(PlanError::BadCommand {
                task: t.id.clone(),
                why: "command must have 1 to 32 arguments",
            });
        }
        if t.command
            .iter()
            .any(|a| a.len() > MAX_ARG_BYTES || a.contains('\0'))
        {
            return Err(PlanError::BadCommand {
                task: t.id.clone(),
                why: "argument too long or contains NUL",
            });
        }
        if t.command[0].is_empty() {
            return Err(PlanError::BadCommand {
                task: t.id.clone(),
                why: "empty program name",
            });
        }
        if t.depends_on.len() > MAX_DEPS_PER_TASK {
            return Err(PlanError::TooManyDependencies(t.id.clone()));
        }
        let timeout = t.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
        if !(1..=MAX_TIMEOUT_SECS).contains(&timeout) {
            return Err(PlanError::BadLimit("timeout_secs must be 1 to 3600"));
        }
        let retries = t.max_retries.unwrap_or(0);
        if retries > MAX_RETRIES {
            return Err(PlanError::BadLimit("max_retries must be 0 to 3"));
        }
        let joined = t.command.join(" ");
        if !detect_secrets(&joined).is_empty() {
            return Err(PlanError::SecretInCommand(t.id.clone()));
        }
        let call = ToolCallRequested {
            tool: "bash".into(),
            command: Some(joined),
            paths: Vec::new(),
            destructive: false,
        };
        let (mut needs_approval, mut reason) = match guardian.preflight(&call) {
            Decision::Deny { reason } => {
                return Err(PlanError::Denied {
                    task: t.id.clone(),
                    reason,
                })
            }
            Decision::RequireApproval { reason } => (true, reason),
            _ => (false, String::new()),
        };
        if t.require_approval && !needs_approval {
            needs_approval = true;
            reason = "approval required by the plan".into();
        }
        tasks.push(NewTask {
            key: t.id.clone(),
            description: t.description.clone(),
            argv: t.command.clone(),
            timeout_secs: timeout,
            max_retries: retries,
            needs_approval,
            approval_reason: reason,
            deps: t.depends_on.clone(),
        });
    }
    for t in &tasks {
        for d in &t.deps {
            if *d == t.key {
                return Err(PlanError::SelfDependency(t.key.clone()));
            }
            if !seen.contains(&d.as_str()) {
                return Err(PlanError::UnknownDependency {
                    task: t.key.clone(),
                    dep: d.clone(),
                });
            }
        }
        let mut uniq = t.deps.clone();
        uniq.sort();
        uniq.dedup();
        if uniq.len() != t.deps.len() {
            return Err(PlanError::BadLimit("duplicate dependency entries"));
        }
    }
    if topo_order(&tasks).is_none() {
        return Err(PlanError::Cycle(find_cycle(&tasks)));
    }
    Ok(ValidPlan {
        name: plan.name.clone(),
        max_parallel,
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<ValidPlan, PlanError> {
        validate(&parse_plan(json)?, &Guardian::default())
    }

    fn t(id: &str, deps: &[&str]) -> String {
        format!(
            r#"{{"id":"{id}","command":["git","status"],"depends_on":{}}}"#,
            serde_json::to_string(deps).unwrap()
        )
    }

    fn plan(tasks: &[String]) -> String {
        format!(r#"{{"name":"p","tasks":[{}]}}"#, tasks.join(","))
    }

    #[test]
    fn valid_dag_gets_a_dependency_first_order() {
        let p = parse(&plan(&[t("c", &["a", "b"]), t("a", &[]), t("b", &["a"])])).unwrap();
        assert_eq!(p.order(), vec!["a", "b", "c"]);
        assert_eq!(p.max_parallel, DEFAULT_PARALLEL);
        assert!(p.tasks.iter().all(|t| !t.needs_approval));
    }

    #[test]
    fn cycles_are_detected_and_reported_with_the_path() {
        let e = parse(&plan(&[t("a", &["c"]), t("b", &["a"]), t("c", &["b"])])).unwrap_err();
        match e {
            PlanError::Cycle(path) => {
                assert_eq!(path.first(), path.last());
                assert!(path.len() >= 4, "{path:?}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse(&plan(&[t("a", &["a"])])).unwrap_err(),
            PlanError::SelfDependency("a".into())
        );
    }

    #[test]
    fn structural_errors_are_rejected() {
        assert!(matches!(
            parse(&plan(&[t("a", &["zzz"])])),
            Err(PlanError::UnknownDependency { .. })
        ));
        assert_eq!(
            parse(&plan(&[t("a", &[]), t("a", &[])])).unwrap_err(),
            PlanError::Duplicate("a".into())
        );
        assert!(matches!(
            parse(&plan(&[t("a b", &[])])),
            Err(PlanError::BadKey(_))
        ));
        assert!(matches!(
            parse(&plan(&[t("../x", &[])])),
            Err(PlanError::BadKey(_))
        ));
        assert_eq!(
            parse(r#"{"name":"p","tasks":[]}"#).unwrap_err(),
            PlanError::Empty
        );
        assert!(matches!(
            parse(r#"{"name":"p","tasks":[],"extra":1}"#),
            Err(PlanError::Parse(_))
        ));
        assert!(matches!(parse("not json"), Err(PlanError::Parse(_))));
        let many: Vec<String> = (0..=MAX_TASKS_PER_RUN)
            .map(|i| t(&format!("t{i}"), &[]))
            .collect();
        assert_eq!(parse(&plan(&many)).unwrap_err(), PlanError::TooManyTasks);
        assert!(matches!(
            parse(r#"{"name":"p","max_parallel":9,"tasks":[{"id":"a","command":["x"]}]}"#),
            Err(PlanError::BadLimit(_))
        ));
        assert!(matches!(
            parse(r#"{"name":"p","tasks":[{"id":"a","command":["x"],"timeout_secs":0}]}"#),
            Err(PlanError::BadLimit(_))
        ));
        assert!(matches!(
            parse(r#"{"name":"p","tasks":[{"id":"a","command":[]}]}"#),
            Err(PlanError::BadCommand { .. })
        ));
    }

    #[test]
    fn destructive_commands_need_approval_denied_ones_invalidate_the_plan() {
        let p = parse(
            r#"{"name":"p","tasks":[
                {"id":"a","command":["git","push","--force","origin","main"]},
                {"id":"b","command":["rm","-rf","build"]},
                {"id":"c","command":["git","status"],"require_approval":true},
                {"id":"d","command":["git","status"]}]}"#,
        )
        .unwrap();
        let flags: Vec<bool> = p.tasks.iter().map(|t| t.needs_approval).collect();
        assert_eq!(flags, vec![true, true, true, false]);
        assert_eq!(p.tasks[2].approval_reason, "approval required by the plan");
        let denied = parse(r#"{"name":"p","tasks":[{"id":"a","command":["cat",".env"]}]}"#);
        assert!(
            matches!(denied, Err(PlanError::Denied { .. })),
            "{denied:?}"
        );
    }

    #[test]
    fn secrets_in_commands_are_refused() {
        let tok = format!("ghp_{}", "a1B2".repeat(9));
        let e = parse(&format!(
            r#"{{"name":"p","tasks":[{{"id":"a","command":["curl","-H","{tok}"]}}]}}"#
        ))
        .unwrap_err();
        assert_eq!(e, PlanError::SecretInCommand("a".into()));
    }
}
