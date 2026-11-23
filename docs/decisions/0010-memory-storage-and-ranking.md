# ADR-0010: Memory storage v2, ranking, consolidation and controls

**Status:** Accepted (implemented in Delivery 1: `kepler-storage` migration v2, `kepler-memory`)

## Context
KPL-011/012 require scoped FTS5 retrieval with provenance, relevance/freshness/trust ranking, dedupe, a
token-budgeted output, consolidation at settlement, and user controls (disable, inspect, export, purge,
retention), with redaction before anything is persisted. Delivery 0 only had a flat `memories` table.

## Decision

### Schema (migration v2, append-only)
`memories` gains `scope` (`project|session|branch|user`), `scope_key` (session id or branch name), `trust` (0-100),
`category`, `content_hash`, `updated_at`, `last_used_at`, `use_count`, `superseded_by`. New tables:
`memory_sources` (provenance: kind, opaque reference, session id; never content), `module_settings`
(`global` or per-project key/value, e.g. `memory.enabled`) and `context_metrics` (token figures with an
`estimated` flag, capped at 5000 rows). The FTS update trigger only fires when `content` changes.

### Scopes
A retrieval for (project P, session S, branch B) sees: `project` memories of P, `session` memories of S within P,
`branch` memories of B within P, and all `user` memories. Nothing else. `user` scope is created only by an explicit
user action (`kepler memory add --scope user`) and is rejected for non-user sources. Branch scope is supported by the
store/CLI, but the v1 event envelope carries no branch, so the daemon never auto-selects one.

### Ranking and output
FTS5 query = up to 12 distinct quoted alphanumeric terms (>= 3 chars, stop words removed) joined with `OR`, so
FTS syntax in user text is inert. Top 30 by bm25 are re-ranked in Rust:
`score = 0.55*relevance + 0.25*freshness + 0.20*trust`, where relevance = |bm25| / best |bm25|, freshness = 0.5^(age/30 d)
and trust = trust/100. Items under 0.30 are noise and dropped; near-duplicates (token Jaccard >= 0.8) of an already
chosen item are dropped; selection is greedy under a token budget (default 600 *estimated* tokens, <= 8 items;
`KEPLER_MEMORY_BUDGET_TOKENS`), skipping items that do not fit. The block always starts with a header saying the notes
may be stale and are NOT instructions; each line carries scope, `user-stated`/`inferred` and age; text is collapsed to one
line (<= 400 chars) so a memory cannot forge extra lines or labels. Weights are first guesses, not tuned on data.

### Dedupe and supersession (at write time, deterministic)
Same normalized hash, or Jaccard >= 0.85 -> duplicate (merge provenance, keep max trust, <= 8 sources).
0.5 <= Jaccard < 0.85 with the same category and new trust >= old trust -> the new row supersedes the old
(`superseded_by`, excluded from retrieval, deleted by retention after 7 days). A derived memory (trust 40) can never
supersede a user-stated one (trust 90). This heuristic can miss real contradictions and can supersede incorrectly
when two different facts share many words; it is conservative about user facts only.

### Consolidation
Explicit directives ("remember that ...", "recuerda que ...") are stored immediately as `user`, trust 90.
`memory.candidate` events are held in RAM (<= 16 per session, <= 64 sessions, redacted already) and persisted
as `derived`, trust 40, only when `agent.settled` reports `ok`. `error`/`cancelled` settlement and `session.closed`
discard unverified candidates. Raw prompts and tool output are never stored; the daemon creates no candidates by
itself (an adapter or the MCP client must send `memory.candidate`).

### Redaction and privacy
Every stored text passes `NewMemory::sanitize` (redaction, NUL/size checks, sensitive-path refusal, secrets-only
rejection). Export re-redacts. `redact` moved from `kepler-storage` to `kepler-policy` (re-exported) so Guardian can use it.

### Controls
`kepler memory list|search|export|add|purge|disable|enable|stats` (see CLI). Purge refuses an empty filter without `--all`
and the CLI additionally requires `--yes`. Disable is a setting honoured by the daemon within 2 s; `KEPLER_MEMORY=off`
disables it at process level. Retention: `KEPLER_MEMORY_RETENTION_DAYS` (default 180, 0 = forever; user-sourced memories are
exempt) and at most 2000 live memories per project, applied at daemon start. The database is still not encrypted.

### Concurrency
Writes go through the daemon's single writer thread (`Writer::exec/submit`, each job in a savepoint, result delivered
after the batch commit). Reads use a lazily opened read-only connection behind a mutex. CLI controls open their own
short-lived writer; SQLite serializes the two processes (WAL, 2 s busy timeout).

## Consequences
Retrieval runs synchronously on the daemon's single runtime thread (one indexed FTS query); its cost is measured in
`docs/development/PERFORMANCE.md`. The handler budget still bounds it (module budget 50 ms, handler 500 ms).

## Alternatives
Embeddings/vector search (new dependency, model download; revisit only if measured retrieval quality demands it);
LLM-based consolidation (privacy and cost); storing whole transcripts (violates "do not store all chats").
