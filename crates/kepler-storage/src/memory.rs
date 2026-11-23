//! Scoped memory store (KPL-011/012): SQL primitives only. Ranking, budgeting and
//! consolidation policy live in `kepler-memory`; everything that persists text is
//! redacted here, in one place, before it reaches the database.
//!
//! Scopes: `project` (default), `session`, `branch` and `user` (explicit, cross-project).
//! A retrieval for project P, session S, branch B sees: project memories of P, session
//! memories of S in P, branch memories of B in P, and every `user` memory.

use crate::redact::redact;
use crate::{Reader, StorageError};
use kepler_policy::sensitive::is_sensitive_path;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;

pub const MAX_MEMORY_TEXT_BYTES: usize = 4096;
pub const TRUST_USER: u8 = 90;
pub const TRUST_DERIVED: u8 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Project,
    Session,
    Branch,
    User,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Session => "session",
            Self::Branch => "branch",
            Self::User => "user",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "project" => Some(Self::Project),
            "session" => Some(Self::Session),
            "branch" => Some(Self::Branch),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

/// A memory to store. `content` is raw; [`NewMemory::sanitize`] redacts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMemory {
    pub project_id: String,
    pub scope: Scope,
    /// Session id (scope `session`) or branch name (scope `branch`); empty otherwise.
    pub scope_key: String,
    pub content: String,
    /// `user` (the user said it) or `derived` (inferred/observed). Never promoted silently.
    pub source: String,
    pub trust: u8,
    pub category: Option<String>,
    /// Provenance: `user_directive`, `candidate`, `settlement`, `cli`...
    pub source_kind: String,
    /// Event id or other opaque reference (never content).
    pub reference: String,
    pub session_id: String,
    /// File the fact came from, if any. Sensitive paths are refused.
    pub source_path: Option<String>,
}

impl NewMemory {
    pub fn new(project_id: &str, content: &str, source: &str) -> Self {
        Self {
            project_id: project_id.to_string(),
            scope: Scope::Project,
            scope_key: String::new(),
            content: content.to_string(),
            source: source.to_string(),
            trust: if source == "user" {
                TRUST_USER
            } else {
                TRUST_DERIVED
            },
            category: None,
            source_kind: "candidate".into(),
            reference: String::new(),
            session_id: String::new(),
            source_path: None,
        }
    }

    /// Validate and redact; returns the text that is allowed to be stored.
    pub fn sanitize(&self) -> Result<String, StorageError> {
        if let Some(p) = &self.source_path {
            if is_sensitive_path(p) {
                return Err(StorageError::Rejected(
                    "source path is on the sensitive-path denylist",
                ));
            }
        }
        if self.source != "user" && self.source != "derived" {
            return Err(StorageError::Rejected("source must be `user` or `derived`"));
        }
        if self.scope == Scope::User && self.source != "user" {
            return Err(StorageError::Rejected(
                "user-scope memories must come from the user",
            ));
        }
        if matches!(self.scope, Scope::Session | Scope::Branch) && self.scope_key.is_empty() {
            return Err(StorageError::Rejected("scope key required"));
        }
        if self.content.len() > MAX_MEMORY_TEXT_BYTES {
            return Err(StorageError::Rejected("memory too large"));
        }
        if self.content.contains('\0') {
            return Err(StorageError::Rejected("memory contains NUL"));
        }
        let clean = redact(&self.content);
        if clean.trim().is_empty() {
            return Err(StorageError::Rejected("empty memory"));
        }
        // A memory that is nothing but redaction markers carries no information.
        if clean.replace(crate::redact::MARK, "").trim().is_empty() {
            return Err(StorageError::Rejected("memory consisted only of secrets"));
        }
        Ok(clean)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreOutcome {
    Inserted {
        id: i64,
    },
    /// Same fact already stored: counters/provenance merged, nothing new inserted.
    Duplicate {
        id: i64,
    },
    /// Inserted and replaced older, similar facts of the same scope and category.
    Superseded {
        id: i64,
        replaced: Vec<i64>,
    },
}

impl StoreOutcome {
    pub fn id(&self) -> i64 {
        match self {
            Self::Inserted { id } | Self::Duplicate { id } | Self::Superseded { id, .. } => *id,
        }
    }
}

/// Lowercased alphanumeric tokens (the unit for hashing and similarity).
pub fn tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Stable 64-bit FNV-1a of the normalized text (same facts differing in case/punctuation collide).
pub fn content_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for t in tokens(text) {
        for b in t.bytes().chain(std::iter::once(b' ')) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

pub fn jaccard(a: &[String], b: &[String]) -> f64 {
    let sa: HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: HashSet<&str> = b.iter().map(String::as_str).collect();
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count() as f64;
    inter / ((sa.len() + sb.len()) as f64 - inter)
}

const DUPLICATE_SIMILARITY: f64 = 0.85;
const SUPERSEDE_SIMILARITY: f64 = 0.5;

const STOPWORDS: &[&str] = &[
    "the", "and", "for", "that", "this", "with", "from", "are", "was", "has", "have", "not", "but",
    "you", "your", "our", "all", "any", "can", "use", "its", "into", "than", "then", "will",
    "should", "would", "could", "para", "que", "los", "las", "una", "uno", "con", "por", "del",
    "como", "pero", "esto", "esta",
];

/// FTS5 OR-query from free text: at most 12 distinct quoted alphanumeric terms of length >= 3,
/// stop words removed. FTS syntax in the input can never be interpreted. `None` = nothing to search.
pub fn fts_query(text: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let terms: Vec<String> = tokens(text)
        .into_iter()
        .filter(|t| t.chars().count() >= 3 && !STOPWORDS.contains(&t.as_str()))
        .filter(|t| seen.insert(t.clone()))
        .take(12)
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

/// Insert a memory with dedupe and supersession. `content` must be the sanitized text.
///
/// Rules (deterministic):
/// 1. same normalized hash in the same scope           -> `Duplicate` (merge provenance, keep max trust)
/// 2. token Jaccard >= 0.85 with a live memory         -> `Duplicate`
/// 3. 0.5 <= Jaccard < 0.85, same category, and the new trust >= old trust -> new row replaces old
///    (old kept for audit with `superseded_by`, excluded from retrieval). A derived memory
///    (trust 40) can therefore never supersede a user-stated one (trust 90).
/// 4. otherwise a plain insert.
pub fn store_memory(
    conn: &Connection,
    m: &NewMemory,
    content: &str,
) -> Result<StoreOutcome, StorageError> {
    let hash = content_hash(content);
    let scope = m.scope.as_str();
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM memories WHERE project_id = ?1 AND scope = ?2 AND scope_key = ?3
               AND content_hash = ?4 AND superseded_by IS NULL LIMIT 1",
            params![m.project_id, scope, m.scope_key, hash],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        merge_duplicate(conn, id, m)?;
        return Ok(StoreOutcome::Duplicate { id });
    }

    let new_tokens = tokens(content);
    let mut replace: Vec<i64> = Vec::new();
    if let Some(q) = fts_query(content) {
        let mut stmt = conn.prepare(
            "SELECT m.id, m.content, m.trust, m.category
             FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
             WHERE memories_fts MATCH ?1 AND m.project_id = ?2 AND m.scope = ?3 AND m.scope_key = ?4
               AND m.superseded_by IS NULL
             ORDER BY bm25(memories_fts) LIMIT 8",
        )?;
        let rows = stmt.query_map(params![q, m.project_id, scope, m.scope_key], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, old, old_trust, old_cat) = row?;
            let sim = jaccard(&new_tokens, &tokens(&old));
            if sim >= DUPLICATE_SIMILARITY {
                merge_duplicate(conn, id, m)?;
                return Ok(StoreOutcome::Duplicate { id });
            }
            if sim >= SUPERSEDE_SIMILARITY
                && old_cat == m.category
                && i64::from(m.trust) >= old_trust
            {
                replace.push(id);
            }
        }
    }

    conn.execute(
        "INSERT INTO memories (project_id, content, source, scope, scope_key, trust, category, content_hash, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            m.project_id,
            content,
            m.source,
            scope,
            m.scope_key,
            i64::from(m.trust),
            m.category,
            hash
        ],
    )?;
    let id = conn.last_insert_rowid();
    add_source(conn, id, m)?;
    for old in &replace {
        conn.execute(
            "UPDATE memories SET superseded_by = ?1 WHERE id = ?2",
            params![id, old],
        )?;
    }
    Ok(if replace.is_empty() {
        StoreOutcome::Inserted { id }
    } else {
        StoreOutcome::Superseded {
            id,
            replaced: replace,
        }
    })
}

fn add_source(conn: &Connection, id: i64, m: &NewMemory) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO memory_sources (memory_id, kind, reference, session_id) VALUES (?1, ?2, ?3, ?4)",
        params![id, m.source_kind, m.reference, m.session_id],
    )?;
    Ok(())
}

fn merge_duplicate(conn: &Connection, id: i64, m: &NewMemory) -> Result<(), StorageError> {
    conn.execute(
        "UPDATE memories SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
           trust = MAX(trust, ?2) WHERE id = ?1",
        params![id, i64::from(m.trust)],
    )?;
    // Keep provenance bounded: at most 8 sources per memory.
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memory_sources WHERE memory_id = ?1",
        [id],
        |r| r.get(0),
    )?;
    if n < 8 {
        add_source(conn, id, m)?;
    }
    Ok(())
}

/// A retrieval request (scopes are enforced in SQL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryQuery {
    pub project_id: String,
    pub session_id: Option<String>,
    pub branch: Option<String>,
    pub text: String,
    /// Candidates to fetch before ranking (capped at 50).
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: i64,
    pub content: String,
    pub scope: Scope,
    pub source: String,
    pub trust: u8,
    pub category: Option<String>,
    /// FTS5 bm25 (lower = more relevant, usually negative).
    pub bm25: f64,
    pub age_days: f64,
    pub use_count: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryRow {
    pub id: i64,
    pub project_id: String,
    pub scope: Scope,
    pub scope_key: String,
    pub content: String,
    pub source: String,
    pub trust: u8,
    pub category: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub use_count: i64,
    pub superseded_by: Option<i64>,
    pub sources: Vec<SourceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRow {
    pub kind: String,
    pub reference: String,
    pub session_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryFilter {
    pub project_id: Option<String>,
    pub scope: Option<Scope>,
    pub include_superseded: bool,
    pub limit: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryStats {
    pub total: i64,
    pub live: i64,
    pub superseded: i64,
    pub user_sourced: i64,
    pub sources: i64,
}

impl Reader {
    /// Scoped FTS5 candidates, best bm25 first. Superseded memories are never returned.
    pub fn memory_candidates(&self, q: &MemoryQuery) -> Result<Vec<Candidate>, StorageError> {
        let Some(fts) = fts_query(&q.text) else {
            return Ok(Vec::new());
        };
        let mut stmt = self.conn().prepare(
            "SELECT m.id, m.content, m.scope, m.source, m.trust, m.category, bm25(memories_fts),
                    julianday('now') - julianday(COALESCE(m.updated_at, m.created_at)), m.use_count
             FROM memories_fts JOIN memories m ON m.id = memories_fts.rowid
             WHERE memories_fts MATCH ?1 AND m.superseded_by IS NULL
               AND ( m.scope = 'user'
                  OR (m.project_id = ?2 AND ( m.scope = 'project'
                       OR (m.scope = 'session' AND m.scope_key = ?3)
                       OR (m.scope = 'branch'  AND m.scope_key = ?4) )) )
             ORDER BY bm25(memories_fts) LIMIT ?5",
        )?;
        let rows = stmt.query_map(
            params![
                fts,
                q.project_id,
                q.session_id.as_deref().unwrap_or("\u{1}none"),
                q.branch.as_deref().unwrap_or("\u{1}none"),
                q.limit.clamp(1, 50)
            ],
            |r| {
                let scope: String = r.get(2)?;
                let trust: i64 = r.get(4)?;
                Ok(Candidate {
                    id: r.get(0)?,
                    content: r.get(1)?,
                    scope: Scope::parse(&scope).unwrap_or(Scope::Project),
                    source: r.get(3)?,
                    trust: u8::try_from(trust.clamp(0, 100)).unwrap_or(0),
                    category: r.get(5)?,
                    bm25: r.get(6)?,
                    age_days: r.get::<_, f64>(7)?.max(0.0),
                    use_count: r.get(8)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn list_memories(&self, f: &MemoryFilter) -> Result<Vec<MemoryRow>, StorageError> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, scope, scope_key, content, source, trust, category, created_at,
                    COALESCE(updated_at, created_at), use_count, superseded_by
             FROM memories
             WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR scope = ?2)
               AND (?3 = 1 OR superseded_by IS NULL)
             ORDER BY id DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![
                f.project_id,
                f.scope.map(Scope::as_str),
                i64::from(f.include_superseded),
                f.limit.clamp(1, 1000)
            ],
            |r| {
                let scope: String = r.get(2)?;
                let trust: i64 = r.get(6)?;
                Ok(MemoryRow {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    scope: Scope::parse(&scope).unwrap_or(Scope::Project),
                    scope_key: r.get(3)?,
                    content: r.get(4)?,
                    source: r.get(5)?,
                    trust: u8::try_from(trust.clamp(0, 100)).unwrap_or(0),
                    category: r.get(7)?,
                    created_at: r.get(8)?,
                    updated_at: r.get(9)?,
                    use_count: r.get(10)?,
                    superseded_by: r.get(11)?,
                    sources: Vec::new(),
                })
            },
        )?;
        let mut out: Vec<MemoryRow> = rows.collect::<Result<_, _>>()?;
        let mut src = self.conn().prepare(
            "SELECT kind, reference, session_id, created_at FROM memory_sources
             WHERE memory_id = ?1 ORDER BY id LIMIT 8",
        )?;
        for row in &mut out {
            row.sources = src
                .query_map([row.id], |r| {
                    Ok(SourceRow {
                        kind: r.get(0)?,
                        reference: r.get(1)?,
                        session_id: r.get(2)?,
                        created_at: r.get(3)?,
                    })
                })?
                .collect::<Result<_, _>>()?;
        }
        Ok(out)
    }

    pub fn memory_stats(&self, project_id: Option<&str>) -> Result<MemoryStats, StorageError> {
        Ok(self.conn().query_row(
            "SELECT COUNT(*), COALESCE(SUM(superseded_by IS NULL), 0),
                    COALESCE(SUM(superseded_by IS NOT NULL), 0),
                    COALESCE(SUM(source = 'user'), 0),
                    (SELECT COUNT(*) FROM memory_sources s JOIN memories m2 ON m2.id = s.memory_id
                      WHERE ?1 IS NULL OR m2.project_id = ?1)
             FROM memories WHERE ?1 IS NULL OR project_id = ?1",
            params![project_id],
            |r| {
                Ok(MemoryStats {
                    total: r.get(0)?,
                    live: r.get(1)?,
                    superseded: r.get(2)?,
                    user_sourced: r.get(3)?,
                    sources: r.get(4)?,
                })
            },
        )?)
    }

    /// Setting for `scope` (a project id or `global`), if any.
    pub fn setting(&self, scope: &str, key: &str) -> Result<Option<String>, StorageError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT value FROM module_settings WHERE scope = ?1 AND key = ?2",
                params![scope, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Most recent metrics, newest first.
    pub fn recent_metrics(&self, limit: u32) -> Result<Vec<MetricRow>, StorageError> {
        let mut stmt = self.conn().prepare(
            "SELECT ts, project_id, session_id, kind, tokens, items, estimated
             FROM context_metrics ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.clamp(1, 200)], |r| {
            Ok(MetricRow {
                ts: r.get(0)?,
                project_id: r.get(1)?,
                session_id: r.get(2)?,
                kind: r.get(3)?,
                tokens: r.get(4)?,
                items: r.get(5)?,
                estimated: r.get::<_, i64>(6)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricRow {
    pub ts: String,
    pub project_id: String,
    pub session_id: String,
    pub kind: String,
    pub tokens: i64,
    pub items: i64,
    /// `true` = Kepler's heuristic estimate; `false` = reported by the harness.
    pub estimated: bool,
}

/// Bump usage counters of memories that were actually injected.
pub fn mark_used(conn: &Connection, ids: &[i64]) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "UPDATE memories SET use_count = use_count + 1,
           last_used_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?1",
    )?;
    for id in ids.iter().take(50) {
        stmt.execute([id])?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeFilter {
    pub project_id: Option<String>,
    pub scope: Option<Scope>,
    pub session_id: Option<String>,
    pub older_than_days: Option<u32>,
    pub superseded_only: bool,
    /// Required to purge with no other filter (guards against an empty filter wiping everything).
    pub all: bool,
}

impl PurgeFilter {
    fn is_unfiltered(&self) -> bool {
        self.project_id.is_none()
            && self.scope.is_none()
            && self.session_id.is_none()
            && self.older_than_days.is_none()
            && !self.superseded_only
    }
}

/// Delete memories (and their provenance). Returns the number of memories removed.
pub fn purge_memories(conn: &Connection, f: &PurgeFilter) -> Result<usize, StorageError> {
    if f.is_unfiltered() && !f.all {
        return Err(StorageError::Rejected(
            "refusing to purge everything without an explicit `all`",
        ));
    }
    let older = f.older_than_days.map(|d| format!("-{d} days"));
    let where_clause = "(?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR scope = ?2)
         AND (?3 IS NULL OR (scope = 'session' AND scope_key = ?3))
         AND (?4 IS NULL OR COALESCE(updated_at, created_at) < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?4))
         AND (?5 = 0 OR superseded_by IS NOT NULL)";
    let args = params![
        f.project_id,
        f.scope.map(Scope::as_str),
        f.session_id,
        older,
        i64::from(f.superseded_only)
    ];
    conn.execute(
        &format!(
            "DELETE FROM memory_sources WHERE memory_id IN (SELECT id FROM memories WHERE {where_clause})"
        ),
        args,
    )?;
    Ok(conn.execute(&format!("DELETE FROM memories WHERE {where_clause}"), args)?)
}

/// Retention: drop memories not updated for `retention_days` (user-sourced memories are kept
/// unless `include_user`), superseded rows after 7 days, and keep at most `max_live` live
/// memories per project (oldest, lowest trust first). Returns rows removed.
pub fn apply_retention(
    conn: &Connection,
    retention_days: Option<u32>,
    max_live_per_project: u32,
    include_user: bool,
) -> Result<usize, StorageError> {
    let mut removed = 0usize;
    removed += conn.execute(
        "DELETE FROM memories WHERE superseded_by IS NOT NULL
           AND COALESCE(updated_at, created_at) < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-7 days')",
        [],
    )?;
    if let Some(d) = retention_days {
        removed += conn.execute(
            "DELETE FROM memories WHERE COALESCE(updated_at, created_at) < strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)
               AND (?2 = 1 OR source <> 'user')",
            params![format!("-{d} days"), i64::from(include_user)],
        )?;
    }
    removed += conn.execute(
        "DELETE FROM memories WHERE id IN (
           SELECT id FROM (
             SELECT id, ROW_NUMBER() OVER (PARTITION BY project_id ORDER BY trust DESC, COALESCE(updated_at, created_at) DESC) AS rn
             FROM memories WHERE superseded_by IS NULL)
           WHERE rn > ?1)",
        [max_live_per_project],
    )?;
    conn.execute(
        "DELETE FROM memory_sources WHERE memory_id NOT IN (SELECT id FROM memories)",
        [],
    )?;
    Ok(removed)
}

pub fn set_setting(
    conn: &Connection,
    scope: &str,
    key: &str,
    value: &str,
) -> Result<(), StorageError> {
    if scope.is_empty() || key.is_empty() || value.len() > 256 {
        return Err(StorageError::Rejected("invalid setting"));
    }
    conn.execute(
        "INSERT INTO module_settings (scope, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT(scope, key) DO UPDATE SET value = excluded.value",
        params![scope, key, value],
    )?;
    Ok(())
}

pub fn record_metric(
    conn: &Connection,
    project_id: &str,
    session_id: &str,
    kind: &str,
    tokens: u32,
    items: u32,
    estimated: bool,
) -> Result<(), StorageError> {
    conn.execute(
        "INSERT INTO context_metrics (project_id, session_id, kind, tokens, items, estimated)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            project_id,
            session_id,
            kind,
            tokens,
            items,
            i64::from(estimated)
        ],
    )?;
    // Bounded: keep the newest 5000 rows.
    conn.execute(
        "DELETE FROM context_metrics WHERE id <= (SELECT MAX(id) FROM context_metrics) - 5000",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrate;

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c
    }

    fn put(c: &Connection, m: &NewMemory) -> StoreOutcome {
        let clean = m.sanitize().unwrap();
        store_memory(c, m, &clean).unwrap()
    }

    fn mem(project: &str, text: &str, source: &str) -> NewMemory {
        NewMemory::new(project, text, source)
    }

    #[test]
    fn jaccard_values() {
        let a = tokens("the build uses cargo workspaces");
        assert!((jaccard(&a, &a) - 1.0).abs() < 1e-9);
        let b = tokens("cargo workspaces");
        let j = jaccard(&a, &b);
        assert!(j > 0.3 && j < 0.6, "{j}");
        assert_eq!(jaccard(&a, &[]), 0.0);
    }

    #[test]
    fn hash_ignores_case_and_punctuation() {
        assert_eq!(
            content_hash("Use pnpm, not npm!"),
            content_hash("use PNPM not npm")
        );
        assert_ne!(content_hash("use pnpm"), content_hash("use npm"));
    }

    #[test]
    fn fts_query_is_inert() {
        assert_eq!(fts_query("*"), None);
        let q = fts_query("fix the parser NEAR/3 cargo OR \"x\" ) (").unwrap();
        assert!(q.starts_with("\"fix\""), "{q}");
        assert!(!q.contains("NEAR/"), "{q}");
        assert!(q.split(" OR ").count() <= 12);
    }

    #[test]
    fn duplicates_merge_and_keep_provenance() {
        let c = db();
        let a = put(&c, &mem("p", "The CI runs on Windows and Linux", "derived"));
        let b = put(
            &c,
            &mem("p", "the ci runs on windows and linux.", "derived"),
        );
        assert!(matches!(a, StoreOutcome::Inserted { .. }));
        assert_eq!(b, StoreOutcome::Duplicate { id: a.id() });
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let s: i64 = c
            .query_row("SELECT COUNT(*) FROM memory_sources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(s, 2);
        // near duplicate (J >= 0.85)
        let d = put(
            &c,
            &mem("p", "The CI runs on Windows and Linux machines", "derived"),
        );
        assert_eq!(d.id(), a.id(), "{d:?}");
    }

    #[test]
    fn newer_fact_supersedes_older_of_same_trust() {
        let c = db();
        let old = put(
            &c,
            &mem("p", "package manager is npm for the web app", "derived"),
        );
        let new = put(
            &c,
            &mem("p", "package manager is pnpm for the web app", "derived"),
        );
        match &new {
            StoreOutcome::Superseded { replaced, .. } => assert_eq!(replaced, &vec![old.id()]),
            o => panic!("{o:?}"),
        }
        let live: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE superseded_by IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 1);
    }

    #[test]
    fn derived_never_supersedes_user_facts() {
        let c = db();
        let user = put(
            &c,
            &mem("p", "package manager is pnpm for the web app", "user"),
        );
        let derived = put(
            &c,
            &mem("p", "package manager is npm for the web app", "derived"),
        );
        assert!(
            matches!(derived, StoreOutcome::Inserted { .. }),
            "{derived:?}"
        );
        let live: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM memories WHERE superseded_by IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(live, 2);
        let tr: i64 = c
            .query_row(
                "SELECT trust FROM memories WHERE id = ?1",
                [user.id()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tr, i64::from(TRUST_USER));
    }

    #[test]
    fn scopes_do_not_leak_across_projects_sessions_or_branches() {
        let c = db();
        let mut p1 = mem("p1", "cargo workspace layout notes", "user");
        let mut s1 = mem("p1", "cargo session scratch idea", "derived");
        s1.scope = Scope::Session;
        s1.scope_key = "sess-A".into();
        let mut b1 = mem("p1", "cargo branch feature flag note", "derived");
        b1.scope = Scope::Branch;
        b1.scope_key = "feat/x".into();
        let p2 = mem("p2", "cargo other project secret plan", "user");
        let mut u = mem("p2", "cargo user wide preference", "user");
        u.scope = Scope::User;
        p1.session_id = "x".into();
        for m in [&p1, &s1, &b1, &p2, &u] {
            put(&c, m);
        }
        let r = Reader::from_connection(c);
        let ids = |q: MemoryQuery| -> Vec<String> {
            r.memory_candidates(&q)
                .unwrap()
                .into_iter()
                .map(|c| c.content)
                .collect()
        };
        let q = |sess: Option<&str>, br: Option<&str>| MemoryQuery {
            project_id: "p1".into(),
            session_id: sess.map(Into::into),
            branch: br.map(Into::into),
            text: "cargo".into(),
            limit: 20,
        };
        let plain = ids(q(None, None));
        assert!(plain.iter().any(|t| t.contains("workspace layout")));
        assert!(plain.iter().any(|t| t.contains("user wide")));
        assert!(!plain.iter().any(|t| t.contains("session scratch")));
        assert!(!plain.iter().any(|t| t.contains("branch feature")));
        assert!(!plain.iter().any(|t| t.contains("other project")));
        let with_s = ids(q(Some("sess-A"), None));
        assert!(with_s.iter().any(|t| t.contains("session scratch")));
        let other_s = ids(q(Some("sess-B"), Some("feat/x")));
        assert!(!other_s.iter().any(|t| t.contains("session scratch")));
        assert!(other_s.iter().any(|t| t.contains("branch feature")));
    }

    #[test]
    fn sanitize_redacts_and_rejects() {
        let m = mem(
            "p",
            "deploy with api_key=sk-abcdefghijklmnopqrstuvwxyz0123456789 on friday",
            "user",
        );
        let clean = m.sanitize().unwrap();
        assert!(!clean.contains("sk-abcdef"), "{clean}");
        assert!(clean.contains("deploy"));
        let pw = mem("p", "login with password=hunter2hunter2 locally", "user")
            .sanitize()
            .unwrap();
        assert!(!pw.contains("hunter2"), "{pw}");
        assert!(mem("p", "[REDACTED]", "user").sanitize().is_err());
        let mut s = mem("p", "x y z", "derived");
        s.source_path = Some(".env".into());
        assert!(s.sanitize().is_err());
        let mut u = mem("p", "global pref", "derived");
        u.scope = Scope::User;
        assert!(u.sanitize().is_err());
        let mut sess = mem("p", "note", "user");
        sess.scope = Scope::Session;
        assert!(sess.sanitize().is_err());
        assert!(mem("p", "bad\0nul", "user").sanitize().is_err());
        assert!(mem("p", &"x ".repeat(3000), "user").sanitize().is_err());
        assert!(mem("p", "ok", "bogus").sanitize().is_err());
    }

    #[test]
    fn purge_requires_a_filter_or_explicit_all() {
        let c = db();
        put(&c, &mem("p1", "alpha note one", "user"));
        put(&c, &mem("p2", "beta note two", "user"));
        assert!(purge_memories(&c, &PurgeFilter::default()).is_err());
        let n = purge_memories(
            &c,
            &PurgeFilter {
                project_id: Some("p1".into()),
                ..PurgeFilter::default()
            },
        )
        .unwrap();
        assert_eq!(n, 1);
        let left: i64 = c
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
        let srcs: i64 = c
            .query_row("SELECT COUNT(*) FROM memory_sources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(srcs, 1, "provenance of purged memories must go too");
        let fts: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM memories_fts WHERE memories_fts MATCH 'alpha'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fts, 0, "fts index must not keep purged text");
        assert_eq!(
            purge_memories(
                &c,
                &PurgeFilter {
                    all: true,
                    ..PurgeFilter::default()
                }
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn retention_caps_and_expires() {
        let c = db();
        for i in 0..6 {
            put(
                &c,
                &mem(
                    "p",
                    &format!("distinct fact number{i} zebra{i} quartz{i}"),
                    "derived",
                ),
            );
        }
        c.execute(
            "UPDATE memories SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now','-40 days') WHERE id <= 2",
            [],
        )
        .unwrap();
        let removed = apply_retention(&c, Some(30), 100, false).unwrap();
        assert_eq!(removed, 2);
        let removed = apply_retention(&c, None, 3, false).unwrap();
        assert_eq!(removed, 1);
        // user-sourced memories survive time-based retention unless asked
        put(
            &c,
            &mem("p", "user decision about deployment windows", "user"),
        );
        c.execute(
            "UPDATE memories SET updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now','-400 days') WHERE source = 'user'",
            [],
        )
        .unwrap();
        assert_eq!(apply_retention(&c, Some(30), 100, false).unwrap(), 0);
        assert_eq!(apply_retention(&c, Some(30), 100, true).unwrap(), 1);
    }

    #[test]
    fn settings_and_metrics_roundtrip() {
        let c = db();
        set_setting(&c, "global", "memory.enabled", "false").unwrap();
        set_setting(&c, "global", "memory.enabled", "true").unwrap();
        let v: String = c
            .query_row(
                "SELECT value FROM module_settings WHERE scope='global' AND key='memory.enabled'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "true");
        assert!(set_setting(&c, "", "k", "v").is_err());
        record_metric(&c, "p", "s", "derived_context", 120, 3, true).unwrap();
        let est: i64 = c
            .query_row("SELECT estimated FROM context_metrics", [], |r| r.get(0))
            .unwrap();
        assert_eq!(est, 1);
    }
}
