# MVP plan

## Delivery 0 — Foundations
**Goal:** create observable, lightweight event path from Pi into Rust Core.
- Cargo workspace, clap CLI, Tokio dispatcher, policy system
- typed versioned events and capability negotiation
- IPC daemon lifecycle and doctor command
- SQLite migrations with privacy defaults
- Pi extension proof of integration and fixtures

**Exit:** Pi input event reaches Kepler and safely returns; no LLM or browser starts; missing core causes passthrough.

## Delivery 1 — Forge + Memory
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

## Global release gates
- End-to-end automatic behavior verified for each claimed harness.
- CPU/RSS/p95 benchmarks and crash recovery tested.
- Documented privacy/security defaults and user disable controls.
- Installation, update and uninstall tested across supported OS.
- No model API or cloud account required for core base functionality.
