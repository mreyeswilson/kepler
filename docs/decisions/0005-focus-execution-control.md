# ADR-0005: Focus as central execution-control engine

**Status:** Accepted for design

## Context
Coding agents may drift from user intent, hallucinate facts, repeat steps and spend excessive time on small tasks.

## Decision
Focus belongs in Rust Core above the modules. It supervises scope, evidence, adaptive budgets, loops, minimal diffs and completion. Use deterministic low-overhead policies first, with interception limited by each harness's capabilities.

## Consequences
Potentially improves quality and throughput; false positives and latency are risks requiring a controlled benchmark, overrides and auditability.

## Alternatives
Prompt-only discipline (weak enforcement), another always-on supervisor LLM (high latency/tokens), hard global timeouts (may kill useful work).
