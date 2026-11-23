# Module contracts

**Focus** is the central execution-control component. The seven supporting modules have isolated budgets, permissions, explicit outcomes and fallbacks.

## Focus — Execution control
**Trigger:** user input, tool activity, diff observation and completion checks where supported. Keeps the original objective, controls scope drift, requires evidence, detects loops, supervises budgets and recommends stopping once acceptance criteria are met. [Full design](../architecture/FOCUS.md).


## Forge — Prompt Compiler
**Trigger:** every user input, with a fast passthrough classifier. Modes: passthrough, enrich, compile, decompose. Preserve exact original prompt and label all inferred constraints; never silently change requested intent. LLM invocation only when valuable and explicitly available.

## Memory — Context and knowledge
**Trigger:** task start, relevant context request, settlement/checkpoint. FTS5 retrieval scoped by project/session/branch. Consolidation keeps durable decisions with provenance and supersession rules. Do not store all chats or secrets.

## Guardian — Token/context optimization
**Trigger:** context preparation, tool result processing when interceptable. Estimate tokens by model family; collect actual usage only when harness exposes it. Compress redundant output while retaining errors, paths, line numbers and acceptance criteria. No unsupported claims of proxy control.

## Orbit — Multiagent orchestration
**Trigger:** complex task when supported and permitted. DAG scheduler, bounded concurrency, worktrees, child process supervision, retries, cancel, conflict checks and checkpoints. Do not spawn agents merely because installed.

## Sentinel — Code audit
**Trigger:** relevant staged/diff changes or requested review. First deterministic checks, Git diff and existing lints/tests; add Tree-sitter analysis or LLM review when justified. Separate confirmed defects from speculative suggestions.

## Vision — UI inspection
**Trigger:** frontend changes and compatible preview availability. Spawn a TypeScript/Playwright worker only on demand; reuse browser process briefly, collect screenshots/logs, perform overflow and accessibility checks. No resident Chromium.

## Genesis — AI interface designer (later phase)
**Trigger:** design creation or redesign tasks where the user has requested them. Creates structured UI specifications, mockup/preview candidates, component mapping and code plans; uses existing project design tokens and the agent's supported stack. Renderers/workers are on demand. [Full design](GENESIS.md).

## Cross-module requirements
Versioned typed result schemas; privacy filters; explicit confidence/provenance; bounded latency and graceful failure for optional enhancements; no unauthorized destructive operations.
