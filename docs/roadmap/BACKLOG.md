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
| KPL-037 | P2 | Orbit DAG scheduler | Cycle detection, bounded concurrency, git worktree isolation, approvals for destructive actions, tests on temporary repositories |

Each issue should include owner (when assigned), dependencies, security implications, tests and performance targets. IDs are planning identifiers, not existing GitHub issues.

## Status (Rust core on branch `feat/rust-core-d0`, Deliveries 0-5)

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
| KPL-014 | Partial | `kepler mcp serve` (ADR-0012): minimal in-tree JSON-RPC, tools `kepler_doctor`/`kepler_inspect`/`kepler_memory_search`, all read-only and bounded; tested with a scripted client over a real process. **Not tested with a third-party MCP client.** No interception is claimed or possible through MCP. |
| KPL-019 | Partial | `adapters/pi` hooks: input, before_agent_start (labeled system-prompt addition), context (estimate only), tool_call (block on deny/unapproved approval), tool_result (redact), agent_start/agent_settled, session_shutdown; 45 node:test tests and one manual run against the real daemon. **Not verified inside a real Pi session**; package still `private: true` ([ADR-0013](../decisions/0013-pi-automatic-hooks.md)). |
| KPL-015 | Done (shallow) | `kepler-sentinel` + `kepler-git`: path rules in the daemon on `files.changed`, added-line rules over `git diff` from `kepler sentinel audit`, findings tied to file/line/redacted evidence in `audit_findings` (migration v3), `confirmed` vs `suspected`, bounded ([ADR-0015](../decisions/0015-sentinel-and-findings.md)). **Not implemented:** integration with the project's own lint/test runs, AST/Tree-sitter analysis. Rules are simple substrings, unmeasured false-positive rate. |
| KPL-016 | Partial | `kepler-vision` (lazy manager, timeouts+kill, capped output, minimal env, localhost allowlist, fake-worker tests) and the private `workers/vision` Playwright worker (stdio JSON, 15 node:test tests, real-process hello) ([ADR-0016](../decisions/0016-vision-worker.md)). **The worker has never run against a real Chromium** (none was downloaded); the DOM probe, screenshots and request interception are unverified in a browser. Not wired to the daemon or hooks. |
| KPL-018, KPL-020 | Not started | Packaging and the npm pipeline stay blocked (license, real-harness verification). KPL-017 scaffold existed before; publishing stays disabled. |
| KPL-026 | Partial | Focus scope lock: files named in the user input vs changed files, new input restarts the task. The original request is not stored (only file names); explicit acceptance criteria are not tracked ([ADR-0014](../decisions/0014-focus-implementation.md)). |
| KPL-027 | Done (uncalibrated) | Loop detector: identical calls, repeated failures, failure streaks, oscillating edits; a retry after a change is legitimate. Thresholds are hypotheses. |
| KPL-028 | Partial | Adaptive budget on tool calls and elapsed time: warns while files change, `recommend_stop` only when stalled. Only advisories: no cancellation anywhere. Tokens/CPU/RAM are not part of the budget. |
| KPL-029 | Partial | Evidence/completion advisories report *observed* verification after the last change or "unknown". Keyword heuristic; no observed/inferred/unknown labelling of arbitrary agent claims. |
| KPL-030 | Partial | Minimal-change guard: dependency manifests and large change sets. No refactor-proportion analysis, pre-existing user edits are not distinguished. |
| KPL-031 | Partial | Completion gate: `completion.verified` / `evidence.unverified_completion` / `completion.recommend_stop`. Acceptance criteria are never evaluated. |
| KPL-032 | Not started | The with/without-Kepler benchmark suite was not run; Focus overhead and false-positive rates are unmeasured. |
| KPL-033 | Partial | By design Focus only warns (`advisories` on the verdict); blocking stays with the Guardian preflight. The Pi adapter does not display advisories yet. |
| KPL-034..036 | Not started | Genesis is design only ([ADR-0018](../decisions/0018-genesis-deferred-design.md)); no UI exists. |
| KPL-037 | Done (CLI) | Orbit: `kepler-orbit`, `tasks`/`task_dependencies`/`orbit_runs` (migration v4), plan validation with cycle detection, bounded concurrency, git worktree per task with dependency merges and conflict/overlap reporting, approval checkpoints for destructive commands, process-tree kill on timeout/cancel, `kepler orbit ...` ([ADR-0017](../decisions/0017-orbit-dag-scheduler.md)). No agent spawning, no OS resource limits, Unix kill path not executed on Unix. |
