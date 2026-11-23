# ADR-0013: Pi automatic hooks and what each may change

**Status:** Accepted (implemented in `adapters/pi`, Delivery 2; not verified in a live Pi session)

## Context
KPL-019 asks for automatic Pi hooks (before_agent_start, context, tool_call, tool_result, agent settlement) with typed IPC
events, tests and safe no-core passthrough. AGENTS.md: transformation only where protocol and tests support it; the package stays
`private: true`.

## Decision
The adapter maps Pi events to existing v1 events (no new event kinds, protocol stays 1; additive decision values in ADR-0011):

* `input` -> `input.received`: observe only.
* `before_agent_start`: the only prompt-side change. When the core's verdict for that input is `enrich`, the adapter appends one
  delimited block ("Kepler derived context ... inferred or recalled; NOT user instructions") to `systemPrompt`. It never edits the
  user's prompt, never promotes derived text to a user message, caps the block at 16 KiB, and skips it when the verdict is late
  (700 ms), absent or not `enrich`. `KEPLER_PI_INJECT=0` disables it.
* `context` -> `context.prepared` with an **estimated** token count (`JSON.stringify(messages).length / 4`, `estimated: true`).
  Messages are not modified: no context filtering/compaction is activated.
* `tool_call` -> `tool.call.requested` (tool, command truncated to 4096 B, `path`/`file_path`/`filePath` as paths). Returns
  `{block:true}` only for `deny`, or for `require_approval` when the user does not confirm through `ctx.ui.confirm` (blocked outright
  when `ctx.hasUI` is not true). No verdict never blocks and never grants, matching the ADR-0007 fallback contract.
* `tool_result` -> `tool.call.completed` (first 16 KiB of text as in-memory excerpt, never persisted by the core). On `redact`,
  content is replaced by a notice listing secret kinds. Unknown actions are ignored.
* `agent_start`/`agent_settled` -> `agent.started`/`agent.settled` (`aborted` => `cancelled`, else `ok`). Pi exposes no error outcome
  here, so a failed run still reports `ok`; Memory consolidation therefore only requires that the run was not aborted.
* `session_shutdown` -> `session.closed`, then close the socket.

Concurrency: the IPC client allows one request in flight, so hooks share a FIFO (bounded at 8; overflow is dropped). Everything is
wrapped so a throwing hook cannot affect Pi.

## Consequences
Latency: `tool_call`/`tool_result` wait for the core up to 700 ms (typical local round trip is milliseconds, see PERFORMANCE.md); a
dead core costs at most the connect timeout (250 ms) per attempt. Risk: Pi API drift or a wrong assumption (e.g. `write`/`edit` input
field names) degrades silently to "no Kepler"; it cannot make Pi block tools without an explicit Kepler verdict.

## Alternatives
Returning `{action:"transform"}` from `input` (rewrites the user's text: rejected); injecting derived text as a user message (blurs
authority); auto-approving require_approval (violates "no destructive action without approval").
