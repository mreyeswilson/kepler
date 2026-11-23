use crate::analyze::{analyze, Mode};
use kepler_events::{Envelope, Payload};
use kepler_policy::{Enrichment, InputClass, ModuleError, OptionalModule};

/// Forge as a pipeline module (first stage after the original is pinned).
/// Passthrough results produce no output at all, so trivial and specific inputs cost nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct ForgeModule;

impl OptionalModule for ForgeModule {
    fn name(&self) -> &'static str {
        "forge"
    }

    fn on_input(
        &self,
        event: &Envelope,
        _class: InputClass,
    ) -> Result<Vec<Enrichment>, ModuleError> {
        let Payload::InputReceived(input) = &event.payload else {
            return Ok(Vec::new());
        };
        let a = analyze(&input.text);
        if a.mode == Mode::Passthrough || a.spec.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![Enrichment::inferred(a.spec.render())])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kepler_policy::{Decision, PolicyEngine};

    fn input(text: &str) -> Envelope {
        let json = serde_json_like(text);
        Envelope::parse(json.as_bytes()).unwrap()
    }

    fn serde_json_like(text: &str) -> String {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        format!(
            r#"{{"version":1,"id":"e1","type":"input.received","timestamp":"2026-01-02T03:04:05Z","harness":"pi","project_id":"p","session_id":"s","correlation_id":"c","payload":{{"text":"{escaped}"}}}}"#
        )
    }

    #[test]
    fn engine_with_forge_enriches_vague_input_and_passes_the_rest() {
        let mut e = PolicyEngine::default();
        e.register(Box::new(ForgeModule));
        let v = e.evaluate(&input("fix the bug in the login flow please"));
        match v.decision {
            Decision::Enrich { derived } => {
                assert_eq!(derived.len(), 1);
                assert_eq!(derived[0].source, "forge");
                assert_eq!(derived[0].provenance, "inferred");
                assert!(derived[0].text.contains("NOT user instructions"));
            }
            d => panic!("{d:?}"),
        }
        for t in [
            "Fix the failing test in parser.rs",
            "why is the build slow?",
            "ok",
        ] {
            assert!(e.evaluate(&input(t)).decision.is_passthrough(), "{t}");
        }
    }
}
