//! User controls over memory (inspect / export / purge / disable / add), usable without a
//! running daemon: reads use a read-only connection, writes use a short-lived writer
//! (SQLite serializes them with the daemon's writer through WAL + busy timeout).

use kepler_policy::redact::redact;
use kepler_storage::{
    purge_memories, set_setting, MemoryFilter, MemoryQuery, MemoryRow, MemoryStats, NewMemory,
    PurgeFilter, Reader, Scope, StorageError, StoreOutcome, Writer, WriterConfig,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3);

pub struct MemoryAdmin {
    path: PathBuf,
}

impl MemoryAdmin {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn reader(&self) -> Result<Option<Reader>, StorageError> {
        if !self.path.exists() {
            return Ok(None);
        }
        Reader::open(&self.path).map(Some)
    }

    fn writer(&self) -> Result<Writer, StorageError> {
        Writer::start(&self.path, WriterConfig::default())
    }

    pub fn list(&self, f: &MemoryFilter) -> Result<Vec<MemoryRow>, StorageError> {
        match self.reader()? {
            Some(r) => r.list_memories(f),
            None => Ok(Vec::new()),
        }
    }

    pub fn stats(&self, project: Option<&str>) -> Result<MemoryStats, StorageError> {
        match self.reader()? {
            Some(r) => r.memory_stats(project),
            None => Ok(MemoryStats::default()),
        }
    }

    /// Read-only retrieval with the same scoping, ranking and budget as the daemon.
    pub fn search(
        &self,
        project: &str,
        session: Option<&str>,
        branch: Option<&str>,
        text: &str,
        budget_tokens: u32,
    ) -> Result<(Option<String>, crate::rank::Selection), StorageError> {
        let Some(r) = self.reader()? else {
            return Ok((None, crate::rank::Selection::default()));
        };
        let cands = r.memory_candidates(&MemoryQuery {
            project_id: project.to_string(),
            session_id: session.map(str::to_string),
            branch: branch.map(str::to_string),
            text: text.to_string(),
            limit: 30,
        })?;
        let sel = crate::rank::select(
            crate::rank::rank(cands),
            budget_tokens.min(4000),
            8,
            kepler_policy::ModelFamily::Generic,
        );
        Ok((crate::rank::render_block(&sel), sel))
    }

    /// `true` unless disabled for the project or globally (project overrides global).
    pub fn is_enabled(&self, project: Option<&str>) -> Result<bool, StorageError> {
        let Some(r) = self.reader()? else {
            return Ok(true);
        };
        let p = match project {
            Some(p) => r.setting(p, "memory.enabled")?,
            None => None,
        };
        let g = r.setting("global", "memory.enabled")?;
        Ok(p.or(g).as_deref() != Some("false"))
    }

    pub fn set_enabled(&self, project: Option<&str>, enabled: bool) -> Result<(), StorageError> {
        let scope = project.unwrap_or("global").to_string();
        let value = if enabled { "true" } else { "false" };
        self.writer()?.exec(TIMEOUT, move |c| {
            set_setting(c, &scope, "memory.enabled", value)
        })
    }

    /// Store a memory by hand (source `user`, trust 90, provenance `cli`).
    pub fn add(
        &self,
        project: &str,
        scope: Scope,
        scope_key: &str,
        text: &str,
    ) -> Result<StoreOutcome, StorageError> {
        let mut m = NewMemory::new(project, text, "user");
        m.scope = scope;
        m.scope_key = scope_key.to_string();
        if scope == Scope::User {
            m.project_id = String::new();
        }
        m.source_kind = "cli".into();
        self.writer()?.store(m, TIMEOUT)
    }

    /// Delete memories matching the filter. An empty filter needs `all = true`.
    pub fn purge(&self, f: PurgeFilter) -> Result<usize, StorageError> {
        if !self.path.exists() {
            return Ok(0);
        }
        self.writer()?.exec(TIMEOUT, move |c| purge_memories(c, &f))
    }

    /// JSON Lines export (one object per memory). Content is passed through the redactor
    /// again so an export can never contain more than what the store would accept.
    pub fn export_jsonl(&self, f: &MemoryFilter) -> Result<String, StorageError> {
        let mut out = String::new();
        for row in self.list(f)? {
            out.push_str(&row_to_json(&row).to_string());
            out.push('\n');
        }
        Ok(out)
    }
}

pub fn row_to_json(row: &MemoryRow) -> Value {
    json!({
        "id": row.id,
        "project_id": row.project_id,
        "scope": row.scope.as_str(),
        "scope_key": row.scope_key,
        "content": redact(&row.content),
        "source": row.source,
        "trust": row.trust,
        "category": row.category,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "use_count": row.use_count,
        "superseded_by": row.superseded_by,
        "provenance": row.sources.iter().map(|s| json!({
            "kind": s.kind, "reference": s.reference, "session_id": s.session_id, "at": s.created_at
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin() -> MemoryAdmin {
        let d = std::env::temp_dir().join(format!(
            "kepler-admin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&d).unwrap();
        MemoryAdmin::new(d.join("k.db"))
    }

    fn all() -> MemoryFilter {
        MemoryFilter {
            limit: 100,
            ..Default::default()
        }
    }

    #[test]
    fn missing_database_is_empty_not_an_error() {
        let a = admin();
        assert!(a.list(&all()).unwrap().is_empty());
        assert_eq!(a.stats(None).unwrap().total, 0);
        assert!(a.is_enabled(Some("p")).unwrap());
        assert_eq!(
            a.purge(PurgeFilter {
                all: true,
                ..Default::default()
            })
            .unwrap(),
            0
        );
        assert!(
            !a.path().exists(),
            "read-only operations must not create the database"
        );
    }

    #[test]
    fn add_inspect_export_purge_cycle() {
        let a = admin();
        a.add("p1", Scope::Project, "", "release branch is stable-2")
            .unwrap();
        a.add("p1", Scope::Session, "s1", "scratch idea for the session")
            .unwrap();
        a.add("p2", Scope::Project, "", "other project fact here")
            .unwrap();
        assert_eq!(a.stats(Some("p1")).unwrap().total, 2);
        assert_eq!(a.stats(None).unwrap().user_sourced, 3);
        let out = a
            .export_jsonl(&MemoryFilter {
                project_id: Some("p1".into()),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        let lines: Vec<Value> = out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l["project_id"] == "p1"));
        assert_eq!(lines[0]["provenance"][0]["kind"], "cli");
        // purge by project then everything
        assert!(a.purge(PurgeFilter::default()).is_err());
        assert_eq!(
            a.purge(PurgeFilter {
                project_id: Some("p1".into()),
                ..Default::default()
            })
            .unwrap(),
            2
        );
        assert_eq!(a.stats(None).unwrap().total, 1);
        assert_eq!(
            a.purge(PurgeFilter {
                all: true,
                ..Default::default()
            })
            .unwrap(),
            1
        );
        assert!(a.list(&all()).unwrap().is_empty());
    }

    #[test]
    fn disable_and_enable_with_project_override() {
        let a = admin();
        a.set_enabled(None, false).unwrap();
        assert!(!a.is_enabled(Some("p")).unwrap());
        a.set_enabled(Some("p"), true).unwrap();
        assert!(a.is_enabled(Some("p")).unwrap());
        assert!(!a.is_enabled(Some("q")).unwrap());
        a.set_enabled(None, true).unwrap();
        assert!(a.is_enabled(Some("q")).unwrap());
    }

    #[test]
    fn user_scope_is_cross_project_and_export_redacts() {
        let a = admin();
        a.add("p1", Scope::User, "", "I prefer small commits")
            .unwrap();
        let rows = a.list(&all()).unwrap();
        assert_eq!(rows[0].project_id, "");
        assert_eq!(rows[0].scope, Scope::User);
        // Bypass the store's redaction to prove the export re-redacts.
        let w = Writer::start(a.path(), WriterConfig::default()).unwrap();
        w.exec(TIMEOUT, |c| {
            c.execute(
                "INSERT INTO memories (project_id, content, source) VALUES ('p9', 'key api_key=sk-abcdefghijklmnopqrstuvwxyz0123456789 here', 'user')",
                [],
            )
            .map(|_| ())
            .map_err(Into::into)
        })
        .unwrap();
        let out = a.export_jsonl(&all()).unwrap();
        assert!(!out.contains("sk-abcdef"), "{out}");
    }
}
