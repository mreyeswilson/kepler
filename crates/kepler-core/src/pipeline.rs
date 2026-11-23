//! Builds the policy engine in the documented order:
//! original (pinned, never modified) -> Forge -> Memory -> Guardian -> harness.
//!
//! Modules are optional: a disabled or unavailable module is simply not registered, and
//! any runtime failure degrades to the original input (see ADR-0009).

use kepler_forge::ForgeModule;
use kepler_memory::{MemoryConfig, MemoryModule, MemoryService};
use kepler_policy::PolicyEngine;
use kepler_storage::Writer;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Total time optional modules may use per event inside the daemon (the IPC handler budget is 500 ms).
pub const MODULE_BUDGET: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct PipelineConfig {
    pub forge: bool,
    pub memory: bool,
    pub memory_config: MemoryConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            forge: true,
            memory: true,
            memory_config: MemoryConfig::default(),
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
        Self {
            forge: !off("KEPLER_FORGE"),
            memory: !memory_config.force_disabled,
            memory_config,
        }
    }
}

/// Where Memory reads and writes. `None` (no database) means Memory is not registered.
pub type MemoryStorage = Option<(Arc<Writer>, PathBuf)>;

pub fn build_engine(cfg: &PipelineConfig, storage: MemoryStorage) -> PolicyEngine {
    let mut engine = PolicyEngine::new(MODULE_BUDGET);
    if cfg.forge {
        engine.register(Box::new(ForgeModule));
    }
    if cfg.memory {
        if let Some((writer, path)) = storage {
            let service = Arc::new(MemoryService::new(writer, path, cfg.memory_config.clone()));
            service.maintenance();
            engine.register(Box::new(MemoryModule::new(service)));
        }
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
        assert_eq!(e.pipeline(), vec!["forge", "memory"]);
        let e = build_engine(&PipelineConfig::default(), None);
        assert_eq!(e.pipeline(), vec!["forge"]);
        let cfg = PipelineConfig {
            forge: false,
            memory: false,
            ..PipelineConfig::default()
        };
        assert!(build_engine(&cfg, Some((w, path))).pipeline().is_empty());
    }
}
