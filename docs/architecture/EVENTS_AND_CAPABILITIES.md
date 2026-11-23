# Events and capability negotiation

## Event envelope (conceptual)
```json
{
  "version": 1,
  "id": "event-id",
  "type": "input.received",
  "timestamp": "RFC3339",
  "harness": "pi",
  "project_id": "stable-project-id",
  "session_id": "session-id",
  "correlation_id": "task-id",
  "payload": {}
}
```

Payloads use discriminated, typed, bounded variants, not arbitrary untrusted instructions.

## Event kinds
`input.received`, `prompt.prepared`, `context.requested`, `context.prepared`, `tool.call.requested`, `tool.call.completed`, `files.changed`, `agent.started`, `agent.settled`, `memory.candidate`, `session.closed`, `focus.scope_drift`, `focus.loop_suspected`, `focus.budget_exceeded`, `focus.evidence_missing`, `focus.completion_candidate`, `genesis.design_requested`, `genesis.design_prepared`.

## Capability contract
Each adapter advertises:
- `input_transform`
- `context_inject`
- `context_filter`
- `tool_preflight`
- `tool_result_observe`
- `lifecycle_observe`
- `agent_spawn`
- `token_usage_observe`
- `operation_cancel` (if supported)
- `diff_observe` (if supported)

Each capability has an implementation kind (native hook, MCP tool, CLI, unsupported), version and constraints. MCP tool availability **does not** imply input interception.

## Ordering
1. Receive input and preserve original verbatim.
2. Focus pins the original objective and budget; Forge classifies and may suggest enriched instructions.
3. Memory retrieves scoped records.
4. Guardian selects context under budget.
5. Harness performs permitted work while Focus observes scope, evidence, repetition and progress where the integration allows.
6. Sentinel/Vision run only on relevant changes. Genesis runs only on requested design work, not on routine code edits.
7. Focus assesses verified completion; Memory consolidates high-value facts on settlement or safe checkpoints.

## Semantics
- Idempotency keys prevent duplicate writes after retries.
- Bounded queues with explicit backpressure and cancellation.
- Event sources are untrusted until verified.
- Optional-module failure falls back without changing original input.
- Security-sensitive preflight never becomes fail-open.
- Events may be observed but not mutated when harness forbids mutation.

## Implementation status (Delivery 0)
The v1 envelope, the 11 payload kinds, size limits and capability negotiation are implemented in `crates/kepler-events` and tested against the shared fixtures in `fixtures/events/` (see `manifest.json`; TypeScript adapters should consume the same files). The wire protocol that carries events is specified in [ADR-0007](../decisions/0007-ipc-wire-protocol.md). Ordering steps 2-7 above (Forge, Memory, Guardian, Sentinel, Vision) are not implemented; the core currently classifies deterministically and returns passthrough or a tool-preflight verdict.
