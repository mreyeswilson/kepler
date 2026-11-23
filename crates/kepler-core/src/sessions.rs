use kepler_ipc::{ConnectionId, Reject};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub const MAX_SESSIONS_PER_CONNECTION: usize = 16;
pub const MAX_TOTAL_SESSIONS: usize = 1024;

#[derive(Default)]
struct Inner {
    owner: HashMap<String, ConnectionId>,
    by_conn: HashMap<ConnectionId, HashSet<String>>,
}

/// Binds each session id to the connection that first used it.
///
/// This is not authentication (same-user processes are trusted at the transport
/// level); it stops one live connection from speaking as another live session.
#[derive(Default)]
pub struct SessionRegistry {
    inner: Mutex<Inner>,
}

impl SessionRegistry {
    pub fn bind(&self, conn: ConnectionId, session: &str) -> Result<(), Reject> {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match g.owner.get(session) {
            Some(&c) if c == conn => return Ok(()),
            Some(_) => {
                return Err(Reject {
                    code: "session_conflict",
                    message: "session id is bound to another connection".into(),
                })
            }
            None => {}
        }
        if g.owner.len() >= MAX_TOTAL_SESSIONS {
            return Err(Reject {
                code: "session_limit",
                message: "too many live sessions".into(),
            });
        }
        let mine = g.by_conn.entry(conn).or_default();
        if mine.len() >= MAX_SESSIONS_PER_CONNECTION {
            return Err(Reject {
                code: "session_limit",
                message: "too many sessions on this connection".into(),
            });
        }
        mine.insert(session.to_string());
        g.owner.insert(session.to_string(), conn);
        Ok(())
    }

    pub fn release_session(&self, conn: ConnectionId, session: &str) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if g.owner.get(session) == Some(&conn) {
            g.owner.remove(session);
            if let Some(set) = g.by_conn.get_mut(&conn) {
                set.remove(session);
            }
        }
    }

    pub fn release_connection(&self, conn: ConnectionId) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(set) = g.by_conn.remove(&conn) {
            for s in set {
                g.owner.remove(&s);
            }
        }
    }

    pub fn live_sessions(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .owner
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_connection_owns_the_session() {
        let r = SessionRegistry::default();
        r.bind(1, "s").unwrap();
        r.bind(1, "s").unwrap();
        assert_eq!(r.bind(2, "s").unwrap_err().code, "session_conflict");
        r.release_connection(1);
        r.bind(2, "s").unwrap();
    }

    #[test]
    fn limits_are_enforced_and_released() {
        let r = SessionRegistry::default();
        for i in 0..MAX_SESSIONS_PER_CONNECTION {
            r.bind(1, &format!("s{i}")).unwrap();
        }
        assert_eq!(r.bind(1, "extra").unwrap_err().code, "session_limit");
        r.release_session(1, "s0");
        r.bind(1, "extra").unwrap();
        // global cap
        let r = SessionRegistry::default();
        for i in 0..MAX_TOTAL_SESSIONS {
            r.bind(i as u64, &format!("g{i}")).unwrap();
        }
        assert_eq!(
            r.bind(99_999, "one-more").unwrap_err().code,
            "session_limit"
        );
    }

    #[test]
    fn release_by_wrong_connection_is_ignored() {
        let r = SessionRegistry::default();
        r.bind(1, "s").unwrap();
        r.release_session(2, "s");
        assert_eq!(r.live_sessions(), 1);
    }
}
