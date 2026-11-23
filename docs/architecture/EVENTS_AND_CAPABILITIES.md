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
`input.received`, `prompt.prepared`, `context.requested`, `context.prepared`, `tool.call.requested`, `tool.call.completed`, `files.changed`, `agent.started`, `agent.settled`, `memory.candidate`, `session.closed`.

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

Each capability has an implementation kind (native hook, MCP tool, CLI, unsupported), version and constraints. MCP tool availability **does not** imply input interception.

## Ordering
1. Receive input and preserve original verbatim.
2. Forge classifies and may suggest enriched instructions.
3. Memory retrieves scoped records.
4. Guardian selects context under budget.
5. Harness performs permitted work.
6. Sentinel/Vision run only on relevant changes.
7. Memory consolidates high-value facts on settlement or safe checkpoints.

## Semantics
- Idempotency keys prevent duplicate writes after retries.
- Bounded queues with explicit backpressure and cancellation.
- Event sources are untrusted until verified.
- Optional-module failure falls back without changing original input.
- Security-sensitive preflight never becomes fail-open.
- Events may be observed but not mutated when harness forbids mutation.
