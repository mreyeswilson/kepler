use crate::sessions::SessionRegistry;
use kepler_events::{Envelope, EventKind, Payload};
use kepler_ipc::{BoxFuture, ConnectionId, Handler, Reject};
use kepler_policy::{Decision, PolicyEngine, Verdict};
use kepler_storage::{EventRecord, Writer};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The daemon's event handler.
pub struct Core {
    engine: PolicyEngine,
    sessions: SessionRegistry,
    storage: Option<Arc<Writer>>,
    handled: AtomicU64,
}

impl Core {
    /// `storage = None` runs without persistence (optional optimization).
    pub fn new(engine: PolicyEngine, storage: Option<Arc<Writer>>) -> Self {
        Self {
            engine,
            sessions: SessionRegistry::default(),
            storage,
            handled: AtomicU64::new(0),
        }
    }

    pub fn handled_events(&self) -> u64 {
        self.handled.load(Ordering::Relaxed)
    }

    pub fn live_sessions(&self) -> usize {
        self.sessions.live_sessions()
    }

    /// Token metrics, always labeled: Kepler's derived context is an estimate; a harness-reported
    /// `context.prepared` keeps its own `estimated` flag. Non-blocking, best effort.
    fn record_metrics(&self, event: &Envelope, verdict: &Verdict) {
        let Some(w) = &self.storage else { return };
        let (project, session) = (event.project_id.clone(), event.session_id.clone());
        match (&event.payload, &verdict.decision) {
            (_, Decision::Enrich { derived }) => {
                let tokens = verdict.estimated_tokens.unwrap_or(0);
                let items = u32::try_from(derived.len()).unwrap_or(u32::MAX);
                w.submit(move |c| {
                    kepler_storage::record_metric(
                        c,
                        &project,
                        &session,
                        "derived_context",
                        tokens,
                        items,
                        true,
                    )
                });
            }
            (Payload::ContextPrepared(p), _) => {
                let (tokens, items, estimated) = (p.tokens, p.items, p.estimated);
                w.submit(move |c| {
                    kepler_storage::record_metric(
                        c,
                        &project,
                        &session,
                        "harness_context",
                        tokens,
                        items,
                        estimated,
                    )
                });
            }
            _ => {}
        }
    }

    /// Synchronous core of `handle_event`; separate for direct unit testing.
    pub fn process(&self, conn: ConnectionId, event: &Envelope) -> Result<Verdict, Reject> {
        self.sessions.bind(conn, &event.session_id)?;
        let verdict = self.engine.evaluate(event);
        self.handled.fetch_add(1, Ordering::Relaxed);
        if let Some(w) = &self.storage {
            // Never blocks; a full queue just drops the metadata row.
            w.record_event(EventRecord::from_event(event, &verdict));
        }
        self.record_metrics(event, &verdict);
        if event.kind == EventKind::SessionClosed {
            self.sessions.release_session(conn, &event.session_id);
        }
        Ok(verdict)
    }
}

impl Handler for Core {
    fn handle_event(
        &self,
        conn: ConnectionId,
        event: Envelope,
    ) -> BoxFuture<'_, Result<Verdict, Reject>> {
        let res = self.process(conn, &event);
        Box::pin(async move { res })
    }

    fn connection_closed(&self, conn: ConnectionId) {
        self.sessions.release_connection(conn);
    }
}
