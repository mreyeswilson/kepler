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
| KPL-021 | P0 | Installed harness/client discovery | Detect Pi, Claude Code and Claude Desktop read-only; report version and integration limits |
| KPL-022 | P1 | Safe configuration installer | Preview, approval, atomic merge, backup, rollback and uninstall |
| KPL-023 | P1 | Claude Desktop MCP adapter | MCP stdio integration with explicitly limited automatic capabilities |
| KPL-024 | P2 | Desktop extension packaging | Test optional MCPB package and platform compatibility |
| KPL-025 | P2 | Expanded client discovery | Cursor, VS Code/Copilot, Windsurf and generic MCP clients |

| KPL-026 | P0 | Focus task contract and scope lock | Immutable request, explicit acceptance criteria, track scope changes and justified exceptions |
| KPL-027 | P0 | Focus loop and stall detector | Detect redundant failed calls, oscillating diffs and nonprogress without disrupting legitimate retries |
| KPL-028 | P0 | Focus adaptive execution budgets | Time/tool/retry budgets, progress-aware warning and only supported cancellation |
| KPL-029 | P0 | Focus evidence gate | Label observed/inferred/unknown, reject unsupported completion claims |
| KPL-030 | P0 | Focus minimal-change guard | Flag unrelated diffs, refactors and dependencies with reasoned exceptions |
| KPL-031 | P0 | Focus completion gate | Acceptance criteria verification and stop recommendation, without blocking productive work |
| KPL-032 | P0 | Controlled baseline benchmark suite | 30–50 coding tasks compared with/without Kepler; speed, quality, loops, drift and cost |
| KPL-033 | P1 | Focus per-harness policy enforcement | Only warn, approve, block or cancel where compatible native capabilities exist |
| KPL-034 | P2 | Genesis UI design specification | Design context, tokens, layouts, wireframes/mockups and variants with provenance |
| KPL-035 | P2 | Genesis preview and code-generation workflow | Approval, design-to-component mapping, project conventions and supported stacks |
| KPL-036 | P2 | Genesis Vision/Sentinel feedback | Render on demand, compare/validate and bound correction loops |

Each issue should include owner (when assigned), dependencies, security implications, tests and performance targets. IDs are planning identifiers, not existing GitHub issues.

## Status (Delivery 0, Rust core on branch `feat/rust-core-d0`)

Status is only "done" when code **and** tests exist. Measured performance is not claimed (see
[performance notes](../development/PERFORMANCE.md)).

| ID | Status | Notes |
|---|---|---|
| KPL-001 | Done | Cargo workspace, `[workspace.lints]`, rustfmt/clippy config, `.github/workflows/ci.yml` (fmt, clippy `-D warnings`, test on ubuntu/macos/windows). The CI matrix has only been run locally on Windows so far. |
| KPL-002 | Done | `kepler-events`: v1 envelope, bounded typed payloads, strict validation, shared JSON fixtures in `fixtures/events`. |
| KPL-003 | Done (D0 scope) | `kepler-policy`: deterministic engine, passthrough default/fallback, fail-closed tool preflight, minimal rule-based classifier. No enrichment modules exist yet (Forge/Memory are Delivery 1). |
| KPL-004 | Done | 8 capabilities, negotiation, explicit `unsupported` (MCP/CLI never grant interception). |
| KPL-005 | Partial | Unix socket / named pipe with current-user isolation, handshake, limits, timeouts. No persistent reconnecting client: one-shot clients reconnect by lazily starting the daemon. Unix transport code is written but was not executed on Unix by the author (CI will). |
| KPL-006 | Partial | `kepler doctor` reports runtime, IPC endpoint, database and capability contract. Adapter section is static text: harness detection is not implemented. |
| KPL-007 | Done | SQLite WAL + FTS5, versioned migrations, single writer, privacy defaults, redaction, sensitive path denylist. Memory write/search APIs exist but nothing calls them yet (Delivery 1). |
| KPL-008 | Partial | `adapters/pi`: TypeScript IPC client (ADR-0007) and an observe-only `input` hook with passthrough on any failure, tested against a mock core on Linux/Windows CI. **Not verified end to end with a real Pi session**; no transformation, no core launch, private package. |
| KPL-009 | Done | `kepler-forge` classifier (passthrough/enrich/compile/decompose), original kept byte for byte, wired into the daemon pipeline (ADR-0009). Rules are English/Spanish keyword heuristics, not evaluated on real prompts. |
| KPL-010 | Done | Derived spec: user-stated claims only from verbatim quotes, inferred/extracted/unknown labels, never promoted; rendered block is disclaimed and bounded. |
| KPL-011 | Done | `kepler-storage` migration v2 + `kepler-memory`: scoped FTS5 (project/session/branch/user), relevance/freshness/trust ranking, output dedupe, estimated-token budget, provenance (ADR-0010). Ranking weights are untuned; no embeddings. Branch scope is never auto-selected (v1 events carry no branch). |
| KPL-012 | Done | Dedupe/supersession at write time (derived never supersedes user facts), consolidation only on `ok` settlement, controls `kepler memory list/search/export/add/purge/disable/enable/stats`, retention (age + per-project cap), redaction before every write and on export. The daemon creates no candidates by itself: an adapter or MCP client must send `memory.candidate`. |
| KPL-013 | Done | Guardian: fail-closed preflight with expanded denylist, user rules file (tighten-only), `require_approval`/`deny` verdicts (approval itself is the harness UI), secret detection in tool results (`redact` verdict), context budget stage, estimated-vs-measured token labels ([ADR-0011](../decisions/0011-guardian-and-wire-additions.md)). String matching is not a sandbox; no real tokenizer. |
| KPL-014..KPL-020 | Not started | Except KPL-017 (scaffold existed before this delivery); publishing stays disabled. |
