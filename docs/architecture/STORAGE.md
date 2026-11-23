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
