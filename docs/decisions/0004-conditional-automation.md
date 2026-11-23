# ADR-0004: Conditional automation

**Status:** Accepted for design

## Context
The user wants Memory, Forge and other modules to run automatically, yet unnecessary processing increases latency, memory and token cost.

## Decision
Trigger by events, classify locally, set per-operation budgets, skip irrelevant modules, and preserve original user instructions. Use models and workers only when needed.

## Consequences
Requires an explicit policy engine, capability-aware fallback and quality benchmarks; avoids indiscriminate heavy execution.

## Alternatives
Manual CLI commands only (unacceptable UX), always run all modules (wasteful).
