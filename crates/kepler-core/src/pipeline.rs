//! Builds the policy engine in the documented order:
//! original (pinned, never modified) -> Forge -> Memory -> Guardian -> harness.
//! (Guardian = budget stage here; its tool preflight and result scan live in the engine.)
//!
//! Modules are optional: a disabled or unavailable module is simply not registered, and
//! any runtime failure degrades to the original input (see ADR-0009).

use kepler_focus::{FocusConfig, FocusModule};
use kepler_forge::ForgeModule;
use kepler_memory::{MemoryConfig, MemoryModule, MemoryService};
use kepler_policy::{BudgetStage, Guardian, GuardianRules, PolicyEngine};
use kepler_sentinel::SentinelModule;
use kepler_storage::Writer;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Total time optional modules may use per event inside the daemon (the IPC handler budget is 500 ms).
pub const MODULE_BUDGET: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub forge: bool,
    /// Focus supervision advisories (ADR-0014). Optional, fail-safe, never blocks.
    pub focus: bool,
    /// Sentinel path audit on `files.changed` (needs the database; ADR-0015).
    pub sentinel: bool,
    pub focus_config: FocusConfig,
    pub memory: bool,
    /// Context budget stage. The security preflight is always on and cannot be disabled.
    pub guardian_budget: bool,
    pub guardian_rules: GuardianRules,
    /// Project directories (`KEPLER_PROJECT_ROOTS`, path-separator list) used to resolve
    /// relative paths through symlinks/junctions in the Guardian preflight.
    pub project_roots: Vec<PathBuf>,
    pub memory_config: MemoryConfig,
    /// Non-fatal problems found while building the config (e.g. bad rule lines).
    pub warnings: Vec<String>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            forge: true,
            focus: true,
            sentinel: true,
            focus_config: FocusConfig::default(),
            memory: true,
            guardian_budget: true,
            guardian_rules: GuardianRules::default(),
            project_roots: Vec::new(),
            memory_config: MemoryConfig::default(),
            warnings: Vec::new(),
        }
    }
}

impl PipelineConfig {
    /// Defaults overridden by `KEPLER_FORGE=off` and the `KEPLER_MEMORY*` variables.
    pub fn from_env() -> Self {
        let off = |k: &str| {
            std::env::var(k).is_ok_and(|v| {
                matches!(
                    v.trim().to_lowercase().as_str(),
                    "off" | "0" | "false" | "no"
                )
            })
        };
        let memory_config = MemoryConfig::from_env();
        let (guardian_rules, warnings) = load_guardian_rules();
        Self {
            forge: !off("KEPLER_FORGE"),
            focus: !off("KEPLER_FOCUS"),
            sentinel: !off("KEPLER_SENTINEL"),
            focus_config: FocusConfig::from_env(),
            memory: !memory_config.force_disabled,
            guardian_budget: !off("KEPLER_GUARDIAN_BUDGET"),
            guardian_rules,
            project_roots: std::env::var_os("KEPLER_PROJECT_ROOTS")
                .map(|v| {
                    std::env::split_paths(&v)
                        .filter(|p| !p.as_os_str().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            memory_config,
            warnings,
        }
    }
}

/// Rules file: `KEPLER_GUARDIAN_RULES` or `<data dir>/guardian-rules.txt`. A missing file is
/// normal; an unreadable one is reported and ignored (built-in rules stay in force).
pub fn load_guardian_rules() -> (GuardianRules, Vec<String>) {
    let path = std::env::var_os("KEPLER_GUARDIAN_RULES")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            kepler_storage::dirs::data_dir()
                .ok()
                .map(|d| d.join("guardian-rules.txt"))
        });
    let Some(path) = path else {
        return (GuardianRules::default(), Vec::new());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (rules, mut warns) = GuardianRules::parse(&text);
            for w in &mut warns {
                *w = format!("{}: {w}", path.display());
            }
            (rules, warns)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (GuardianRules::default(), Vec::new())
        }
        Err(e) => (
            GuardianRules::default(),
            vec![format!(
                "{}: cannot read guardian rules: {e}",
                path.display()
            )],
        ),
    }
}

/// Where Memory reads and writes. `None` (no database) means Memory is not registered.
pub type MemoryStorage = Option<(Arc<Writer>, PathBuf)>;

pub fn build_engine(cfg: &PipelineConfig, storage: MemoryStorage) -> PolicyEngine {
    let mut engine = PolicyEngine::new(MODULE_BUDGET);
    engine.set_guardian(
        Guardian::new(cfg.guardian_rules.clone()).with_roots(cfg.project_roots.clone()),
    );
    if cfg.forge {
        engine.register(Box::new(ForgeModule));
    }
    if cfg.sentinel {
        if let Some((writer, _)) = &storage {
            engine.register(Box::new(SentinelModule::new(writer.clone())));
        }
    }
    if cfg.memory {
        if let Some((writer, path)) = storage {
            let service = Arc::new(MemoryService::new(writer, path, cfg.memory_config.clone()));
            service.maintenance();
            engine.register(Box::new(MemoryModule::new(service)));
        }
    }
    if cfg.focus {
        engine.register(Box::new(FocusModule::new(cfg.focus_config.clone())));
    }
    if cfg.guardian_budget {
        engine.register_stage(Box::new(BudgetStage::default()));
    }
    engine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_forge_then_memory_and_disabled_modules_are_absent() {
        let d = std::env::temp_dir().join(format!("kepler-pipe-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("k.db");
        let w = Arc::new(Writer::start(&path, Default::default()).unwrap());
        let e = build_engine(&PipelineConfig::default(), Some((w.clone(), path.clone())));
        assert_eq!(
            e.pipeline(),
            vec!["forge", "sentinel", "memory", "focus", "guardian"]
        );
        let e = build_engine(&PipelineConfig::default(), None);
        assert_eq!(e.pipeline(), vec!["forge", "focus", "guardian"]);
        let cfg = PipelineConfig {
            forge: false,
            focus: false,
            sentinel: false,
            memory: false,
            guardian_budget: false,
            ..PipelineConfig::default()
        };
        assert!(build_engine(&cfg, Some((w, path))).pipeline().is_empty());
    }
}
