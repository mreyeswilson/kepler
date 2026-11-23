# Architecture overview

## Components
1. Harness adapters (Pi first) normalize hooks and commands into versioned events.
2. Kepler Core (Rust) hosts event dispatcher, policy engine, execution budgets, module registry and capability negotiation.
3. Modules implement Forge, Memory, Guardian, Orbit, Sentinel and Vision.
4. SQLite persists memories, decisions, sessions and selective event metadata.
5. MCP stdio exposes callable tools; CLI offers manual administration and diagnostics.
6. Optional per-user daemon provides shared state; one-shot CLI is preferred for simple actions.
7. Isolated workers are launched on demand (Playwright, external models, LSP).

```text
Developer -> Coding Harness -> Native Adapter/Hook -> IPC
                                                  |
                                               Kepler Core
                                     Event Bus + Policy Engine
                                       |        |       |
                                 Forge/Memory Guardian  ...
                                       |        |       |
                                     Storage  MCP   Workers
```

## Execution strategy
- Embedded lightweight adapter logic when practical; shared Rust daemon only if required.
- Unix sockets on Unix, named pipes on Windows, owned by the current user.
- Lazy startup and configurable idle shutdown; avoid permanent global pollers.
- Tokio for I/O; bounded blocking worker pool for CPU-bound analysis.
- One SQLite write coordinator; bounded readers and transaction timeouts.
- All queues bounded, cancellable and observable.

## Policy strategy
Route on intent, changed files, trust level, available capabilities, latency budget and risk.
Return passthrough if enrichment fails or budget expires. Never silently bypass a destructive-action permission check.

## Performance targets, not measured results
- Idle daemon RAM: target <30 MB excluding external workers.
- In-process simple event dispatch: target <10 ms.
- Warm small memory retrieval: target <100 ms.
- LLM calls for trivial instructions: zero.

Measure on real repositories and track p50/p95/p99, CPU, peak/idle RSS and total workflow outcomes.

## Expansion boundaries
Core never imports Pi/Claude/Gemini/OpenCode SDKs. Adapter IPC and events are versioned. Worker failures and missing capabilities degrade gracefully.

## Risks
Hook API drift, memory poisoning, accidental prompt transformation, concurrency conflicts, privacy leaks and performance regressions. See [threat model](../security/THREAT_MODEL.md).
