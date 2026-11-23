use crate::rules::audit_paths;
use kepler_events::{Envelope, Payload};
use kepler_policy::{Advisory, AdvisoryLevel, Enrichment, InputClass, ModuleError, OptionalModule};
use kepler_storage::{store_findings, Severity, Writer};
use std::sync::Arc;

/// Sentinel in the daemon: path rules on `files.changed`, persisted to `audit_findings`
/// (fire and forget through the bounded writer queue) and summarized as one advisory when
/// something high or critical is found. It never alters the decision.
pub struct SentinelModule {
    writer: Arc<Writer>,
}

impl SentinelModule {
    pub fn new(writer: Arc<Writer>) -> Self {
        Self { writer }
    }
}

impl OptionalModule for SentinelModule {
    fn name(&self) -> &'static str {
        "sentinel"
    }

    fn on_input(&self, _e: &Envelope, _c: InputClass) -> Result<Vec<Enrichment>, ModuleError> {
        Ok(Vec::new())
    }

    fn advise(&self, event: &Envelope) -> Result<Vec<Advisory>, ModuleError> {
        let Payload::FilesChanged(f) = &event.payload else {
            return Ok(Vec::new());
        };
        let hits = audit_paths(&f.paths);
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let serious = hits.iter().filter(|h| h.severity >= Severity::High).count();
        let findings: Vec<_> = hits
            .into_iter()
            .map(|h| h.into_finding(&event.project_id, &event.session_id))
            .collect();
        self.writer
            .submit(move |c| store_findings(c, &findings).map(|_| ()));
        if serious == 0 {
            return Ok(Vec::new());
        }
        Ok(vec![Advisory::new(
            "sentinel",
            AdvisoryLevel::Warn,
            "audit.high_severity_path",
            format!(
                "{serious} changed path(s) match high-severity audit rules; run `kepler sentinel list` for details."
            ),
        )])
    }
}
