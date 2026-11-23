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
    (
        3,
        "audit_findings",
        r#"
-- Sentinel / Vision results. Evidence is a short redacted excerpt; secret rules store only the kind.
CREATE TABLE audit_findings (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    project_id  TEXT NOT NULL,
    session_id  TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL,
    rule_id     TEXT NOT NULL,
    severity    TEXT NOT NULL CHECK (severity IN ('info', 'low', 'medium', 'high', 'critical')),
    confidence  TEXT NOT NULL CHECK (confidence IN ('confirmed', 'suspected')),
    path        TEXT NOT NULL DEFAULT '',
    line        INTEGER,
    message     TEXT NOT NULL,
    evidence    TEXT NOT NULL DEFAULT '',
    fingerprint TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'dismissed')),
    UNIQUE (project_id, fingerprint)
);
CREATE INDEX audit_findings_project ON audit_findings (project_id, status, id);
"#,
    ),
    (
        4,
        "orbit_tasks",
        r#"
-- Orbit (task DAG scheduler). Commands are stored as an argv JSON array (the planner refuses
-- secrets in them); outputs are never stored: only exit codes and a short redacted error.
CREATE TABLE orbit_runs (
    run_id       TEXT PRIMARY KEY,
    project_id   TEXT NOT NULL,
    repo_path    TEXT NOT NULL,
    base_ref     TEXT NOT NULL,
    name         TEXT NOT NULL,
    status       TEXT NOT NULL,
    max_parallel INTEGER NOT NULL,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE TABLE tasks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL,
    task_key        TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    command         TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('pending', 'running', 'done', 'failed', 'cancelled', 'skipped', 'awaiting_approval', 'denied')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    max_retries     INTEGER NOT NULL DEFAULT 0,
    timeout_secs    INTEGER NOT NULL,
    branch          TEXT,
    worktree        TEXT,
    exit_code       INTEGER,
    error           TEXT,
    changed_files   TEXT,
    approval        TEXT NOT NULL DEFAULT 'none' CHECK (approval IN ('none', 'required', 'granted', 'denied')),
    approval_reason TEXT NOT NULL DEFAULT '',
    started_at      TEXT,
    ended_at        TEXT,
    UNIQUE (run_id, task_key)
);
CREATE TABLE task_dependencies (
    task_id    INTEGER NOT NULL,
    depends_on INTEGER NOT NULL,
    PRIMARY KEY (task_id, depends_on),
    CHECK (task_id <> depends_on)
);
CREATE INDEX tasks_run ON tasks (run_id, id);
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
