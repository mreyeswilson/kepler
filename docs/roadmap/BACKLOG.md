# Initial development backlog

| ID | Priority | Task | Acceptance criteria |
|---|---|---|---|
| KPL-001 | P0 | Rust workspace skeleton | Workspace compiles, rustfmt/clippy/test CI |
| KPL-002 | P0 | Versioned event types | Typed envelopes, schema fixtures, invalid input tests |
| KPL-003 | P0 | Policy engine | Bounded rules with deterministic passthrough |
| KPL-004 | P0 | Capability negotiation | Unsupported features explicitly reported |
| KPL-005 | P0 | Local IPC transport | Current-user isolation, timeouts and reconnect |
| KPL-006 | P0 | CLI doctor | Reports runtime, adapters, database and capabilities |
| KPL-007 | P0 | SQLite schema/migrations | WAL, FTS5, privacy defaults and migration tests |
| KPL-008 | P0 | Pi adapter input hook | Normal instruction reaches core, graceful fallback |
| KPL-009 | P1 | Forge input classifier | Pass-through cases preserved verbatim |
| KPL-010 | P1 | Forge derived spec | Assumptions labeled, no fabricated requirements |
| KPL-011 | P1 | Memory retrieval | Scoped FTS5 with relevance/provenance tests |
| KPL-012 | P1 | Memory consolidation | Duplicate/stale facts handled safely |
| KPL-013 | P1 | Guardian context budgets | Measured vs estimated token usage separated |
| KPL-014 | P1 | MCP stdio | Discovery and calls tested with compatible client |
| KPL-015 | P2 | Sentinel diff analysis | Findings tied to changed files and evidence |
| KPL-016 | P2 | Vision worker | On-demand Playwright with timeout/cleanup |
| KPL-017 | P0 | Pi npm package scaffold | Manifest, explicit extension path and CI dry run; publishing disabled |
| KPL-018 | P1 | Native binary packaging | Verified binaries and cross-platform installation strategy |
| KPL-019 | P1 | Pi automatic hook integration | Typed event IPC, tests and safe no-core passthrough |
| KPL-020 | P1 | npm public release pipeline | Verified scope, license, trusted publishing, approval and version gate |

Each issue should include owner (when assigned), dependencies, security implications, tests and performance targets. IDs are planning identifiers, not existing GitHub issues.
