use crate::StorageError;
use rusqlite::Connection;

/// Ordered, append-only list. Never edit a released migration; add a new one.
const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "initial",
        r#"
CREATE TABLE sessions (
    session_id  TEXT PRIMARY KEY,
    harness     TEXT NOT NULL,
    project_id  TEXT NOT NULL,
    first_seen  TEXT NOT NULL,
    last_seen   TEXT NOT NULL,
    event_count INTEGER NOT NULL DEFAULT 0,
    closed      INTEGER NOT NULL DEFAULT 0
);

-- Metadata only. There is deliberately no column for prompt text, command lines or tool output.
CREATE TABLE events (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id       TEXT NOT NULL,
    kind           TEXT NOT NULL,
    ts             TEXT NOT NULL,
    harness        TEXT NOT NULL,
    project_id     TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    payload_bytes  INTEGER NOT NULL,
    decision       TEXT NOT NULL,
    reason         TEXT,
    received_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (session_id, event_id)
);
CREATE INDEX events_session ON events (session_id, id);
CREATE INDEX events_project_ts ON events (project_id, ts);

CREATE TABLE memories (
    id         INTEGER PRIMARY KEY,
    project_id TEXT NOT NULL,
    content    TEXT NOT NULL,
    source     TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX memories_project ON memories (project_id);

CREATE VIRTUAL TABLE memories_fts USING fts5 (
    content, content = 'memories', content_rowid = 'id'
);
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts (rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;
CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, content) VALUES ('delete', old.id, old.content);
    INSERT INTO memories_fts (rowid, content) VALUES (new.id, new.content);
END;

CREATE TABLE schema_migrations (
    version    INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
"#,
    ),
    (
        2,
        "memory_scopes_provenance_settings_metrics",
        r#"
ALTER TABLE memories ADD COLUMN scope TEXT NOT NULL DEFAULT 'project';
ALTER TABLE memories ADD COLUMN scope_key TEXT NOT NULL DEFAULT '';
ALTER TABLE memories ADD COLUMN trust INTEGER NOT NULL DEFAULT 40;
ALTER TABLE memories ADD COLUMN category TEXT;
ALTER TABLE memories ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE memories ADD COLUMN updated_at TEXT;
ALTER TABLE memories ADD COLUMN last_used_at TEXT;
ALTER TABLE memories ADD COLUMN use_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN superseded_by INTEGER;
UPDATE memories SET updated_at = created_at WHERE updated_at IS NULL;
CREATE INDEX memories_scope ON memories (project_id, scope, scope_key);
CREATE INDEX memories_hash ON memories (project_id, scope, scope_key, content_hash);

-- Only re-index when the text changes (usage counters are updated on every retrieval).
DROP TRIGGER memories_au;
CREATE TRIGGER memories_au AFTER UPDATE OF content ON memories BEGIN
    INSERT INTO memories_fts (memories_fts, rowid, content) VALUES ('delete', old.id, old.content);
    INSERT INTO memories_fts (rowid, content) VALUES (new.id, new.content);
END;

-- Where a memory came from. Never contains prompts or tool output, only identifiers.
CREATE TABLE memory_sources (
    id         INTEGER PRIMARY KEY,
    memory_id  INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    reference  TEXT NOT NULL DEFAULT '',
    session_id TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX memory_sources_memory ON memory_sources (memory_id);

CREATE TABLE module_settings (
    scope TEXT NOT NULL,
    key   TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (scope, key)
);

-- Token numbers are always labeled: estimated = 1 (Kepler heuristic) or 0 (harness measurement).
CREATE TABLE context_metrics (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    project_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    kind       TEXT NOT NULL,
    tokens     INTEGER NOT NULL,
    items      INTEGER NOT NULL DEFAULT 0,
    estimated  INTEGER NOT NULL
);
CREATE INDEX context_metrics_session ON context_metrics (session_id, id);
"#,
    ),
];

pub const LATEST_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;

/// Apply pending migrations, each in its own transaction. Refuses databases
/// from the future instead of guessing.
pub fn migrate(conn: &mut Connection) -> Result<(), StorageError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > LATEST_VERSION {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported: LATEST_VERSION,
        });
    }
    for (version, name, sql) in MIGRATIONS.iter().filter(|(v, _, _)| *v > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name) VALUES (?1, ?2)",
            (version, name),
        )?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn fresh_database_reaches_latest() {
        let mut c = mem();
        migrate(&mut c).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, LATEST_VERSION);
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut c = mem();
        migrate(&mut c).unwrap();
        migrate(&mut c).unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, MIGRATIONS.len() as i64);
    }

    #[test]
    fn newer_database_is_refused() {
        let mut c = mem();
        c.pragma_update(None, "user_version", LATEST_VERSION + 1)
            .unwrap();
        assert!(matches!(
            migrate(&mut c),
            Err(StorageError::SchemaTooNew { .. })
        ));
    }

    #[test]
    fn failed_migration_rolls_back() {
        let mut c = mem();
        c.execute_batch("CREATE TABLE sessions (x INTEGER);")
            .unwrap(); // collides with v1
        assert!(migrate(&mut c).is_err());
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 0);
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'events'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "partial migration must not persist");
    }

    #[test]
    fn fts5_is_available_and_triggers_sync() {
        let mut c = mem();
        migrate(&mut c).unwrap();
        c.execute("INSERT INTO memories (project_id, content, source) VALUES ('p', 'uses cargo workspaces', 'user')", []).unwrap();
        let hits: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'cargo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
        c.execute("DELETE FROM memories", []).unwrap();
        let hits: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'cargo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 0);
    }

    #[test]
    fn events_table_has_no_raw_content_columns() {
        let mut c = mem();
        migrate(&mut c).unwrap();
        let mut stmt = c.prepare("PRAGMA table_info(events)").unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for banned in ["text", "prompt", "command", "output", "payload", "content"] {
            assert!(
                !cols.iter().any(|c| c == banned),
                "{banned} must not be stored"
            );
        }
    }
}
