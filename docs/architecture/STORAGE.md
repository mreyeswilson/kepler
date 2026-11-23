# Local storage

## Choice
SQLite with WAL and FTS5 initially; optional embeddings later if measured retrieval quality benefits. No mandatory external database, service or sync account.

## Logical tables
- projects: stable repo/worktree identity and settings
- harnesses: versions and capability snapshots
- sessions: agent sessions and lifecycle
- events: selected metadata only, bounded retention
- memories: scoped, deduplicated contextual knowledge
- memory_sources: provenance and evidence
- decisions: current/replaced architectural choices
- tasks, task_dependencies: orchestration DAG
- tool_executions: selective diagnostics
- audit_findings: findings and their status
- context_metrics: measured or estimated usage, explicitly labeled
- module_settings: per-project and global config

## Memory retrieval
Query normalization -> FTS5 -> scope filter -> relevance/freshness/trust ranking -> deduplication -> budgeted output. Respect repo, branch, user and session boundaries.

## Writes
Single coordinator, short transactions, busy timeouts, bounded read pool and versioned migrations. SQLite WAL permits concurrent readers but only one active writer.

## Privacy
Exclude .env, credentials, tokens and configured sensitive locations before indexing. Filter and redact outputs before persistence. Minimize raw prompts and tool outputs; offer purge/export and retention controls.

## Paths
Use platform configuration/data directories and restrictive local file permissions; do not place user-global databases inside repositories by default.

## Implementation status (Delivery 0)
Implemented in `crates/kepler-storage` (migration v1): `sessions`, `events` (metadata only: no prompt, command or tool output columns, idempotent on `(session_id, event_id)`, bounded retention), `memories` + `memories_fts` (FTS5, external content, trigger-synced, scoped by `project_id`, content redacted, sensitive source paths refused). One writer thread with a bounded queue that drops on overflow; read-only connections for queries. Data directory: `KEPLER_HOME` or `%LOCALAPPDATA%\kepler` / `~/Library/Application Support/kepler` / `$XDG_DATA_HOME/kepler`. Not implemented: the remaining logical tables, purge/export commands, retention settings, encryption (the database is **not** encrypted). Dependency rationale: [ADR-0008](../decisions/0008-storage-dependencies.md).
