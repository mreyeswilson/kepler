//! Memory service: lazy read-only connection, scoped retrieval, pending candidates and
//! consolidation. Every persisted text goes through `NewMemory::sanitize` (redaction)
//! before it is queued to the single writer.

use crate::rank::{rank, render_block, select, Selection};
use kepler_policy::ModelFamily;
use kepler_storage::{
    apply_retention, mark_used, record_metric, store_memory, MemoryQuery, NewMemory, Reader,
    StorageError, StoreOutcome, Writer, TRUST_DERIVED, TRUST_USER,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct MemoryConfig {
    /// Token budget for one retrieval (estimated tokens).
    pub budget_tokens: u32,
    pub max_items: usize,
    /// Candidates fetched from FTS before ranking.
    pub fetch_limit: u32,
    pub max_pending_per_session: usize,
    pub max_pending_sessions: usize,
    /// `None` = no time-based expiry (the per-project cap still applies).
    pub retention_days: Option<u32>,
    pub max_live_per_project: u32,
    pub family: ModelFamily,
    /// Hard off switch (`KEPLER_MEMORY=off`).
    pub force_disabled: bool,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            budget_tokens: 600,
            max_items: 8,
            fetch_limit: 30,
            max_pending_per_session: 16,
            max_pending_sessions: 64,
            retention_days: Some(180),
            max_live_per_project: 2000,
            family: ModelFamily::Generic,
            force_disabled: false,
        }
    }
}

impl MemoryConfig {
    /// Defaults overridden by `KEPLER_MEMORY` (off/0/false/no), `KEPLER_MEMORY_BUDGET_TOKENS`
    /// and `KEPLER_MEMORY_RETENTION_DAYS` (0 = keep forever).
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(v) = std::env::var("KEPLER_MEMORY") {
            c.force_disabled = matches!(
                v.trim().to_lowercase().as_str(),
                "off" | "0" | "false" | "no"
            );
        }
        if let Some(n) = env_u32("KEPLER_MEMORY_BUDGET_TOKENS") {
            c.budget_tokens = n.clamp(50, 4000);
        }
        if let Some(n) = env_u32("KEPLER_MEMORY_RETENTION_DAYS") {
            c.retention_days = (n > 0).then_some(n);
        }
        c
    }
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok()?.trim().parse().ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ctx {
    pub project_id: String,
    pub session_id: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
struct Pending {
    text: String,
    category: Option<String>,
    reference: String,
    /// Branch-scoped candidate (key = branch name); project scope otherwise.
    branch_scope: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Retrieval {
    pub block: Option<String>,
    pub selection: Selection,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Consolidation {
    pub queued: usize,
    pub rejected: usize,
}

const SETTINGS_TTL: Duration = Duration::from_secs(2);
const WRITE_TIMEOUT: Duration = Duration::from_millis(250);

pub struct MemoryService {
    writer: Arc<Writer>,
    db_path: PathBuf,
    cfg: MemoryConfig,
    reader: Mutex<Option<Reader>>,
    pending: Mutex<PendingStore>,
    enabled_cache: Mutex<HashMap<String, (Instant, bool)>>,
}

#[derive(Default)]
struct PendingStore {
    by_session: HashMap<String, (u64, Vec<Pending>)>,
    tick: u64,
}

impl MemoryService {
    pub fn new(writer: Arc<Writer>, db_path: PathBuf, cfg: MemoryConfig) -> Self {
        Self {
            writer,
            db_path,
            cfg,
            reader: Mutex::new(None),
            pending: Mutex::new(PendingStore::default()),
            enabled_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &MemoryConfig {
        &self.cfg
    }

    fn with_reader<T>(
        &self,
        f: impl FnOnce(&Reader) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut guard = self
            .reader
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            *guard = Some(Reader::open(&self.db_path)?);
        }
        let res = match guard.as_ref() {
            Some(r) => f(r),
            None => Err(StorageError::WriterUnavailable),
        };
        if res.is_err() {
            *guard = None; // reopen next time
        }
        res
    }

    /// Memory is on unless forced off by env, or disabled by the user globally or for the
    /// project (`kepler memory disable`). Settings are cached for 2 s.
    pub fn enabled(&self, project_id: &str) -> bool {
        if self.cfg.force_disabled {
            return false;
        }
        {
            let cache = self
                .enabled_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((at, v)) = cache.get(project_id) {
                if at.elapsed() < SETTINGS_TTL {
                    return *v;
                }
            }
        }
        let v = self
            .with_reader(|r| {
                let project = r.setting(project_id, "memory.enabled")?;
                let global = r.setting("global", "memory.enabled")?;
                // A project setting overrides the global one.
                let flag = project.or(global);
                Ok(flag.as_deref() != Some("false"))
            })
            .unwrap_or(true);
        let mut cache = self
            .enabled_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(project_id.to_string(), (Instant::now(), v));
        v
    }

    /// Scoped, ranked, budgeted retrieval. Records usage and an *estimated* token metric
    /// without blocking.
    pub fn retrieve(
        &self,
        ctx: &Ctx,
        query_text: &str,
        budget_tokens: u32,
    ) -> Result<Retrieval, StorageError> {
        let start = Instant::now();
        let q = MemoryQuery {
            project_id: ctx.project_id.clone(),
            session_id: Some(ctx.session_id.clone()),
            branch: ctx.branch.clone(),
            text: query_text.to_string(),
            limit: self.cfg.fetch_limit,
        };
        let cands = self.with_reader(|r| r.memory_candidates(&q))?;
        let budget = budget_tokens.min(4000);
        let selection = select(rank(cands), budget, self.cfg.max_items, self.cfg.family);
        let block = render_block(&selection);
        if !selection.items.is_empty() {
            let ids: Vec<i64> = selection.items.iter().map(|r| r.candidate.id).collect();
            let (p, s, t, n) = (
                ctx.project_id.clone(),
                ctx.session_id.clone(),
                selection.tokens,
                selection.items.len() as u32,
            );
            self.writer.submit(move |conn| {
                mark_used(conn, &ids)?;
                record_metric(conn, &p, &s, "memory_recall", t, n, true)
            });
        }
        Ok(Retrieval {
            block,
            selection,
            elapsed: start.elapsed(),
        })
    }

    /// Query-less session-start brief of the project's durable memories (ADR-0027), scoped to
    /// (project, branch) and bounded by `budget_tokens` (estimated) and `max_items`.
    pub fn brief(&self, ctx: &Ctx, budget_tokens: u32) -> Result<Retrieval, StorageError> {
        let start = Instant::now();
        let cands =
            self.with_reader(|r| r.memory_brief(&ctx.project_id, ctx.branch.as_deref(), 30))?;
        let budget = budget_tokens.min(4000);
        let selection = select(rank(cands), budget, self.cfg.max_items, self.cfg.family);
        let block = crate::rank::render_brief(&selection);
        if !selection.items.is_empty() {
            let ids: Vec<i64> = selection.items.iter().map(|r| r.candidate.id).collect();
            let (p, s, t, n) = (
                ctx.project_id.clone(),
                ctx.session_id.clone(),
                selection.tokens,
                selection.items.len() as u32,
            );
            self.writer.submit(move |conn| {
                mark_used(conn, &ids)?;
                record_metric(conn, &p, &s, "memory_brief", t, n, true)
            });
        }
        Ok(Retrieval {
            block,
            selection,
            elapsed: start.elapsed(),
        })
    }

    /// Persist an explicit user directive ("remember that ...") right away (non-blocking).
    /// Returns `false` when it was rejected by sanitization (secrets only, too large...).
    pub fn remember_directive(&self, ctx: &Ctx, text: &str, reference: &str) -> bool {
        let mut m = NewMemory::new(&ctx.project_id, text, "user");
        m.trust = TRUST_USER;
        m.source_kind = "user_directive".into();
        m.reference = reference.to_string();
        m.session_id = ctx.session_id.clone();
        self.submit_store(m)
    }

    /// Queue a candidate until the next settlement. Bounded per session and in total.
    pub fn queue_candidate(
        &self,
        ctx: &Ctx,
        text: &str,
        category: Option<&str>,
        reference: &str,
    ) -> bool {
        self.queue_scoped(ctx, text, category, reference, false)
    }

    /// Like [`Self::queue_candidate`]; with `branch_scope` the memory is only recalled on
    /// `ctx.branch` (ignored, and project scope used, when the branch is unknown).
    pub fn queue_scoped(
        &self,
        ctx: &Ctx,
        text: &str,
        category: Option<&str>,
        reference: &str,
        branch_scope: bool,
    ) -> bool {
        // Reject early (and never hold unredacted secrets in RAM longer than needed).
        let probe = NewMemory::new(&ctx.project_id, text, "derived");
        let Ok(clean) = probe.sanitize() else {
            return false;
        };
        let mut store = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        store.tick += 1;
        let tick = store.tick;
        if !store.by_session.contains_key(&ctx.session_id)
            && store.by_session.len() >= self.cfg.max_pending_sessions
        {
            if let Some(oldest) = store
                .by_session
                .iter()
                .min_by_key(|(_, (t, _))| *t)
                .map(|(k, _)| k.clone())
            {
                store.by_session.remove(&oldest);
            }
        }
        let entry = store
            .by_session
            .entry(ctx.session_id.clone())
            .or_insert_with(|| (tick, Vec::new()));
        entry.0 = tick;
        if entry.1.len() >= self.cfg.max_pending_per_session {
            return false;
        }
        entry.1.push(Pending {
            text: clean,
            category: category.map(str::to_string),
            reference: reference.to_string(),
            branch_scope: branch_scope && ctx.branch.is_some(),
        });
        true
    }

    pub fn pending_count(&self, session_id: &str) -> usize {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_session
            .get(session_id)
            .map_or(0, |(_, v)| v.len())
    }

    /// Settlement: persist (dedupe/supersede happen in storage) every pending candidate of the session.
    pub fn consolidate(&self, ctx: &Ctx, reference: &str) -> Consolidation {
        let pending = self.take_pending(&ctx.session_id);
        let mut out = Consolidation::default();
        for p in pending {
            let mut m = NewMemory::new(&ctx.project_id, &p.text, "derived");
            m.trust = TRUST_DERIVED;
            m.category = p.category;
            if p.branch_scope {
                if let Some(b) = &ctx.branch {
                    m.scope = kepler_storage::Scope::Branch;
                    m.scope_key = b.clone();
                }
            }
            m.source_kind = "settlement".into();
            m.reference = if p.reference.is_empty() {
                reference.to_string()
            } else {
                p.reference
            };
            m.session_id = ctx.session_id.clone();
            if self.submit_store(m) {
                out.queued += 1;
            } else {
                out.rejected += 1;
            }
        }
        out
    }

    /// Drop candidates that were never verified (error/cancelled settlement, session closed).
    pub fn discard_pending(&self, session_id: &str) -> usize {
        self.take_pending(session_id).len()
    }

    fn take_pending(&self, session_id: &str) -> Vec<Pending> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_session
            .remove(session_id)
            .map(|(_, v)| v)
            .unwrap_or_default()
    }

    fn submit_store(&self, m: NewMemory) -> bool {
        let Ok(clean) = m.sanitize() else {
            return false;
        };
        self.writer
            .submit(move |conn| store_memory(conn, &m, &clean).map(|_: StoreOutcome| ()))
    }

    /// Blocking store (CLI/tests): returns the outcome.
    pub fn store_blocking(&self, m: NewMemory) -> Result<StoreOutcome, StorageError> {
        self.writer.store(m, WRITE_TIMEOUT)
    }

    /// Apply the retention policy on the writer thread (non-blocking).
    pub fn maintenance(&self) {
        let (days, cap) = (self.cfg.retention_days, self.cfg.max_live_per_project);
        self.writer.submit(move |conn| {
            apply_retention(conn, days, cap, false)?;
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kepler_storage::WriterConfig;

    fn dir() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "kepler-mem-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn svc() -> (MemoryService, Arc<Writer>, PathBuf) {
        let d = dir();
        let path = d.join("k.db");
        let w = Arc::new(Writer::start(&path, WriterConfig::default()).unwrap());
        (
            MemoryService::new(w.clone(), path, MemoryConfig::default()),
            w,
            d,
        )
    }

    fn ctx(p: &str, s: &str) -> Ctx {
        Ctx {
            project_id: p.into(),
            session_id: s.into(),
            branch: None,
        }
    }

    fn flush(w: &Writer) {
        assert!(w.flush(Duration::from_secs(2)));
    }

    #[test]
    fn directive_is_stored_redacted_and_retrieved_with_provenance() {
        let (s, w, d) = svc();
        let c = ctx("p1", "s1");
        assert!(s.remember_directive(
            &c,
            "we deploy with token=abcDEF1234567890abcDEF1234567890 every friday using pnpm",
            "evt-1"
        ));
        flush(&w);
        let r = s.retrieve(&c, "how do we deploy with pnpm", 600).unwrap();
        let block = r.block.expect("memory should be recalled");
        assert!(block.contains("pnpm"), "{block}");
        assert!(!block.contains("abcDEF1234567890"), "{block}");
        assert!(block.contains("user-stated"));
        flush(&w);
        // usage + labeled estimated metric were recorded
        let reader = Reader::open(&d.join("k.db")).unwrap();
        let m = reader.recent_metrics(5).unwrap();
        assert!(m.iter().any(|m| m.kind == "memory_recall" && m.estimated));
        let rows = reader
            .list_memories(&kepler_storage::MemoryFilter {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].use_count, 1);
        assert_eq!(rows[0].sources[0].kind, "user_directive");
        assert_eq!(rows[0].sources[0].reference, "evt-1");
    }

    #[test]
    fn retrieval_is_project_scoped() {
        let (s, w, _d) = svc();
        s.remember_directive(&ctx("p1", "s1"), "the database is postgres 16", "e1");
        flush(&w);
        let other = s
            .retrieve(&ctx("p2", "s9"), "which database postgres", 600)
            .unwrap();
        assert!(other.block.is_none());
        let same = s
            .retrieve(&ctx("p1", "s2"), "which database postgres", 600)
            .unwrap();
        assert!(same.block.is_some());
    }

    #[test]
    fn candidates_persist_only_on_ok_settlement() {
        let (s, w, _d) = svc();
        let c = ctx("p1", "s1");
        assert!(s.queue_candidate(
            &c,
            "the linter config lives in tools/lint.toml",
            Some("repo"),
            "e1"
        ));
        assert_eq!(s.pending_count("s1"), 1);
        // nothing retrievable before settlement
        flush(&w);
        assert!(s
            .retrieve(&c, "where is the linter config", 600)
            .unwrap()
            .block
            .is_none());
        let rep = s.consolidate(&c, "settled-1");
        assert_eq!(rep.queued, 1);
        assert_eq!(s.pending_count("s1"), 0);
        flush(&w);
        let b = s
            .retrieve(&c, "where is the linter config", 600)
            .unwrap()
            .block
            .unwrap();
        assert!(
            b.contains("inferred") && b.contains("tools/lint.toml"),
            "{b}"
        );

        // error settlement: discard
        assert!(s.queue_candidate(&c, "flaky unverified claim about caching layer", None, "e2"));
        assert_eq!(s.discard_pending("s1"), 1);
        flush(&w);
        assert!(s
            .retrieve(&c, "caching layer claim", 600)
            .unwrap()
            .block
            .is_none());
    }

    #[test]
    fn duplicate_candidates_are_consolidated_once() {
        let (s, w, d) = svc();
        let c = ctx("p1", "s1");
        for _ in 0..3 {
            s.queue_candidate(&c, "tests run with cargo nextest", None, "e");
            s.consolidate(&c, "set");
        }
        flush(&w);
        let reader = Reader::open(&d.join("k.db")).unwrap();
        let stats = reader.memory_stats(Some("p1")).unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.sources, 3);
    }

    #[test]
    fn pending_is_bounded() {
        let (s, _w, _d) = svc();
        let c = ctx("p1", "s1");
        let mut accepted = 0;
        for i in 0..100 {
            if s.queue_candidate(&c, &format!("fact number{i} about thing{i}"), None, "e") {
                accepted += 1;
            }
        }
        assert_eq!(accepted, s.config().max_pending_per_session);
        for i in 0..200 {
            s.queue_candidate(
                &ctx("p", &format!("sess{i}")),
                "some fact about stuff",
                None,
                "e",
            );
        }
        let total: usize = (0..200).map(|i| s.pending_count(&format!("sess{i}"))).sum();
        assert!(total <= s.config().max_pending_sessions, "{total}");
    }

    #[test]
    fn secrets_only_candidates_are_rejected() {
        let (s, _w, _d) = svc();
        assert!(!s.queue_candidate(&ctx("p", "s"), "[REDACTED]", None, "e"));
        assert!(!s.remember_directive(&ctx("p", "s"), "   ", "e"));
    }

    #[test]
    fn disable_setting_and_env_switch() {
        let (s, w, d) = svc();
        assert!(s.enabled("p1"));
        w.exec(Duration::from_secs(2), |c| {
            kepler_storage::set_setting(c, "global", "memory.enabled", "false")
        })
        .unwrap();
        // cached for up to 2s: use a fresh service to observe the stored value
        let s2 = MemoryService::new(w.clone(), d.join("k.db"), MemoryConfig::default());
        assert!(!s2.enabled("p1"));
        w.exec(Duration::from_secs(2), |c| {
            kepler_storage::set_setting(c, "p1", "memory.enabled", "true")
        })
        .unwrap();
        let s3 = MemoryService::new(w.clone(), d.join("k.db"), MemoryConfig::default());
        assert!(s3.enabled("p1"), "project setting overrides global");
        assert!(!s3.enabled("p2"));
        let off = MemoryService::new(
            w,
            d.join("k.db"),
            MemoryConfig {
                force_disabled: true,
                ..MemoryConfig::default()
            },
        );
        assert!(!off.enabled("p1"));
    }
}
