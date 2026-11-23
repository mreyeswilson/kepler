use crate::{StorageError, BUSY_TIMEOUT};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;

/// Read-only connection (`query_only`); never writes, never migrates.
pub struct Reader {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryHit {
    pub id: i64,
    pub content: String,
    pub rank: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub event_id: String,
    pub kind: String,
    pub session_id: String,
    pub decision: String,
    pub reason: Option<String>,
    pub payload_bytes: i64,
}

impl Reader {
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    #[cfg(test)]
    pub(crate) fn from_connection(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        conn.pragma_update(None, "query_only", "ON")?;
        Ok(Self { conn })
    }

    pub fn event_count(&self) -> Result<i64, StorageError> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?)
    }

    pub fn session_event_count(&self, session_id: &str) -> Result<Option<i64>, StorageError> {
        match self.conn.query_row(
            "SELECT event_count FROM sessions WHERE session_id = ?1",
            [session_id],
            |r| r.get(0),
        ) {
            Ok(n) => Ok(Some(n)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn recent_events(&self, limit: u32) -> Result<Vec<StoredEvent>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, kind, session_id, decision, reason, payload_bytes
             FROM events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.min(200)], |r| {
            Ok(StoredEvent {
                event_id: r.get(0)?,
                kind: r.get(1)?,
                session_id: r.get(2)?,
                decision: r.get(3)?,
                reason: r.get(4)?,
                payload_bytes: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Scoped (per project) FTS5 search. The user query is reduced to quoted
    /// alphanumeric terms, so FTS syntax in it can never be interpreted.
    pub fn search_memories(
        &self,
        project_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<MemoryHit>, StorageError> {
        let terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .take(16)
            .map(|t| format!("\"{t}\""))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, bm25(memories_fts)
             FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
             WHERE memories_fts MATCH ?1 AND m.project_id = ?2
             ORDER BY bm25(memories_fts) LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![terms.join(" "), project_id, limit.min(50)],
            |r| {
                Ok(MemoryHit {
                    id: r.get(0)?,
                    content: r.get(1)?,
                    rank: r.get(2)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// Snapshot for `kepler doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbStatus {
    pub path: String,
    pub exists: bool,
    pub schema_version: Option<i64>,
    pub journal_mode: Option<String>,
    pub fts5: Option<bool>,
    pub size_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Inspect without creating or migrating anything.
pub fn inspect(path: &Path) -> DbStatus {
    let mut s = DbStatus {
        path: path.display().to_string(),
        exists: path.exists(),
        schema_version: None,
        journal_mode: None,
        fts5: None,
        size_bytes: None,
        error: None,
    };
    if !s.exists {
        return s;
    }
    s.size_bytes = std::fs::metadata(path).ok().map(|m| m.len());
    let probe = || -> Result<(i64, String, bool), StorageError> {
        let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        c.busy_timeout(BUSY_TIMEOUT)?;
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let j: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        let fts: i64 = c.query_row("SELECT sqlite_compileoption_used('ENABLE_FTS5')", [], |r| {
            r.get(0)
        })?;
        Ok((v, j, fts == 1))
    };
    match probe() {
        Ok((v, j, f)) => {
            s.schema_version = Some(v);
            s.journal_mode = Some(j);
            s.fts5 = Some(f);
        }
        Err(e) => s.error = Some(e.to_string()),
    }
    s
}
