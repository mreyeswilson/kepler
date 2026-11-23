# MVP plan

## Delivery 0 — Foundations
**Goal:** create observable, lightweight event path from Pi into Rust Core.
- Cargo workspace, clap CLI, Tokio dispatcher, policy system and Focus event contracts
- typed versioned events and capability negotiation
- IPC daemon lifecycle and doctor command
- SQLite migrations with privacy defaults
- Pi extension proof of integration and fixtures
- Pi npm package scaffold and no-publish validation workflow
- document future native binary delivery and npm release gates
- read-only detection architecture for Pi, Claude Code and Claude Desktop
- distinguish native hooks from MCP-only clients; never claim universal automatic interception

**Exit:** Pi input event reaches Kepler and safely returns; no LLM or browser starts; missing core causes passthrough.

## Delivery 1 — Focus + Forge + Memory
- minimal Focus task contract, scope monitoring, evidence labels, repetition detection and completion checkpoints
- adaptive budgets initially warn rather than blindly terminate productive work
- collect baseline outcomes on identical tasks with Kepler disabled
- input classifier with passthrough/enrichment
- preserve original instruction and derived context
- FTS5 scoped memory retrieval with provenance
- selective consolidation on agent settlement and checkpoints
- configurable disable/inspection/export and project controls

**Exit:** users issue normal Pi instructions and get relevant context automatically, without manual commands.

## Delivery 2 — Guardian + MCP
- stdio MCP server, tools with bounded output
- model-aware context estimation and deduplication
- measurements labeled actual vs estimated
- second harness adapter and compatibility matrix

**Exit:** measurable context benefit without quality regression.

## Delivery 3 — Sentinel
- diff-aware risk rules, existing lint/test integration
- prioritization and findings schema

## Delivery 4 — Vision
- demand-started Playwright worker, responsive checks and evidence

## Delivery 5 — Orbit
- DAG scheduler, isolated worktrees, agent lifecycle support

## Delivery 6 — Genesis (post-MVP)
- design intent and context extraction, reusable design tokens and preview contract
- generate design variants and component/code plans using the existing coding agent
- approval checkpoint for material visual/design changes; Vision and Sentinel validate the result
- avoid spawning browser/model workers for non-design tasks

## Global release gates
- With/without Kepler benchmark on representative tasks, without meaningful correctness regression.
- Focus overhead and false-positive supervision alerts are measured.
- End-to-end automatic behavior verified for each claimed harness.
- CPU/RSS/p95 benchmarks and crash recovery tested.
- Documented privacy/security defaults and user disable controls.
- Installation, update and uninstall tested across supported OS.
- No model API or cloud account required for core base functionality.

## Delivery status (branch `feat/rust-core-d0`)
Status follows the backlog rule: "done" only with code **and** tests. Exit criteria below are **not** claimed as met where
they need a real harness, browser or benchmark.

| Delivery | Code and tests | Exit criteria |
|---|---|---|
| 0 Foundations | Implemented | Event reaches core and returns, passthrough on failure: met in tests with a mock core; **not verified in a real Pi session** |
| 1 Focus + Forge + Memory | Forge, Memory implemented; Focus implemented as non-blocking advisories (ADR-0014) | Relevant context automatically: not verified with real Pi; **baseline benchmark (KPL-032) not run**, Focus thresholds uncalibrated |
| 2 Guardian + MCP | Implemented (output compression, second harness adapter and compatibility matrix are **not**) | "Measurable benefit without quality regression" is **not measured** |
| 3 Sentinel | Implemented (path + diff rules, `audit_findings`); lint/test integration **not** | n/a |
| 4 Vision | Manager and private worker implemented; worker **never run against a real browser** | n/a |
| 5 Orbit | Implemented (DAG, worktrees, approvals, CLI); no agent lifecycle beyond supervising child processes | n/a |
| 6 Genesis | Not implemented; design only (ADR-0018) | n/a |

Global release gates (benchmark, end-to-end per harness, cross-OS install tests, crash recovery) remain **open**.
