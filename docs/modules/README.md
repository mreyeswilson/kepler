# Module contracts

**Focus** is the central execution-control component. The seven supporting modules have isolated budgets, permissions, explicit outcomes and fallbacks.

## Focus — Execution control
**Trigger:** user input, tool activity, diff observation and completion checks where supported. Keeps the original objective, controls scope drift, requires evidence, detects loops, supervises budgets and recommends stopping once acceptance criteria are met. [Full design](../architecture/FOCUS.md). *Status:* implemented as non-blocking advisories ([ADR-0014](../decisions/0014-focus-implementation.md)); thresholds uncalibrated.


## Forge — Prompt Compiler
**Trigger:** every user input, with a fast passthrough classifier. Modes: passthrough, enrich, compile, decompose. Preserve exact original prompt and label all inferred constraints; never silently change requested intent. LLM invocation only when valuable and explicitly available. *Status (Delivery 1):* deterministic classifier and derived spec implemented, no LLM path ([ADR-0009](../decisions/0009-module-pipeline-and-forge.md)).

## Memory — Context and knowledge
**Trigger:** task start, relevant context request, settlement/checkpoint. FTS5 retrieval scoped by project/session/branch. Consolidation keeps durable decisions with provenance and supersession rules. Do not store all chats or secrets. *Status (Delivery 1):* implemented, see [ADR-0010](../decisions/0010-memory-storage-and-ranking.md).

## Guardian — Token/context optimization
**Trigger:** context preparation, tool result processing when interceptable. *Status (Delivery 2):* implemented as the tool preflight/result scan plus a budget stage ([ADR-0011](../decisions/0011-guardian-and-wire-additions.md)); compression of tool output is not implemented. Estimate tokens by model family; collect actual usage only when harness exposes it. Compress redundant output while retaining errors, paths, line numbers and acceptance criteria. No unsupported claims of proxy control.

## Orbit — Multiagent orchestration
**Trigger:** complex task when supported and permitted. DAG scheduler, bounded concurrency, worktrees, child process supervision, retries, cancel, conflict checks and checkpoints. Do not spawn agents merely because installed. *Status:* implemented as a command-task scheduler (DAG, worktrees, approvals, `kepler orbit`); it supervises child processes; `agent` tasks run `claude -p` sub-agents, and Claude Code/Desktop can start, supervise and approve runs through MCP tools (`kepler_orbit_*`, [ADR-0030](../decisions/0030-mcp-tools-for-orbit-and-modules.md)) when the user agrees; nothing starts agents on its own ([ADR-0017](../decisions/0017-orbit-dag-scheduler.md)).

## Sentinel — Code audit
**Trigger:** relevant staged/diff changes or requested review. First deterministic checks, Git diff and existing lints/tests; add Tree-sitter analysis or LLM review when justified. Separate confirmed defects from speculative suggestions. *Status:* deterministic path and diff rules implemented ([ADR-0015](../decisions/0015-sentinel-and-findings.md)); no Tree-sitter, LLM review or lint/test integration.

## Vision — UI inspection
**Trigger:** frontend changes and compatible preview availability. Spawn a TypeScript/Playwright worker only on demand; reuse browser process briefly, collect screenshots/logs, perform overflow and accessibility checks. No resident Chromium. *Status:* manager and private worker implemented, off until `kepler vision setup` (or `KEPLER_VISION_WORKER`); run against a real Edge on Windows 11, not yet on Linux/macOS ([ADR-0016](../decisions/0016-vision-worker.md)).

## Genesis — AI interface designer (later phase)
**Trigger:** design creation or redesign tasks where the user has requested them. Creates structured UI specifications, mockup/preview candidates, component mapping and code plans; uses existing project design tokens and the agent's supported stack. Renderers/workers are on demand. [Full design](GENESIS.md). *Status:* not implemented, design only ([ADR-0018](../decisions/0018-genesis-deferred-design.md)).

## Cross-module requirements
Versioned typed result schemas; privacy filters; explicit confidence/provenance; bounded latency and graceful failure for optional enhancements; no unauthorized destructive operations.
