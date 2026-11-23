use crate::sessions::SessionRegistry;
use kepler_events::{Envelope, EventKind};
use kepler_ipc::{BoxFuture, ConnectionId, Handler, Reject};
use kepler_policy::{PolicyEngine, Verdict};
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

    /// Synchronous core of `handle_event`; separate for direct unit testing.
    pub fn process(&self, conn: ConnectionId, event: &Envelope) -> Result<Verdict, Reject> {
        self.sessions.bind(conn, &event.session_id)?;
        let verdict = self.engine.evaluate(event);
        self.handled.fetch_add(1, Ordering::Relaxed);
        if let Some(w) = &self.storage {
            // Never blocks; a full queue just drops the metadata row.
            w.record_event(EventRecord::from_event(event, &verdict));
        }
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
