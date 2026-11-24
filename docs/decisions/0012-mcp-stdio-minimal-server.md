# ADR-0012: MCP stdio server implemented in-tree (no new dependency)

**Status:** Accepted (implemented in Delivery 2: `kepler-mcp`, `kepler mcp serve`). The "no write tools" rule is amended by [ADR-0030](0030-mcp-tools-for-orbit-and-modules.md).

## Context
KPL-014 needs an MCP stdio server with discovery and calls. AGENTS.md forbids new runtime dependencies without an
ADR and asks to prefer a minimal implementation. The official Rust SDK (`rmcp`) brings tokio features, `schemars`,
`futures`, `pin-project`, proc-macros and a large compile/binary cost for what is three read-only tools.

## Decision
Implement the needed subset of MCP (JSON-RPC 2.0, newline-delimited messages on stdin/stdout) in `crates/kepler-mcp`
using only `serde_json` (already a dependency). No new crate is added to the workspace's dependency graph.

* Methods: `initialize` (negotiates 2025-06-18, 2025-03-26 or 2024-11-05, otherwise answers with the newest it supports),
  `notifications/initialized` (no reply), `ping`, `tools/list`, `tools/call`. Anything else -> `-32601`.
  Capabilities advertise `tools` only (no `listChanged`, no resources/prompts/logging/sampling).
* No JSON-RPC batching (removed in 2025-06-18); arrays are rejected with `-32600`.
* Tools (all read-only, output <= 16 KiB, input line <= 1 MiB read with a bounded buffer):
  `kepler_doctor`, `kepler_inspect` (recent event metadata, memory counters, token metrics with `basis` estimated|measured),
  `kepler_memory_search` (same scoping/ranking/budget as the daemon; requires an explicit `project`).
* Tool failures are results with `isError: true`; unknown tools and malformed arguments are `-32602`.
* stdout carries protocol messages only; diagnostics go to stderr. The server exits on stdin EOF.
* The server `instructions` and the docs state what MCP is: callable tools. It cannot intercept prompts, tool calls or
  results, so no Forge/Memory/Guardian automation happens through MCP (that needs a native adapter such as Pi's).
* No write tools on purpose: an agent that can be prompt-injected should not be able to create memories, approve
  anything or purge data through MCP.

Client configuration (generic stdio client): command `kepler`, args `["mcp", "serve"]`.

## Consequences
Smaller binary and attack surface; we own protocol conformance. Verified with unit tests and a scripted JSON-RPC client over
a real `kepler mcp serve` process; **not** verified against a third-party MCP client (Claude Desktop, MCP Inspector).
Spec drift (new protocol revisions) must be tracked by hand; the SUPPORTED_PROTOCOLS list is the single place to extend.

## Alternatives
`rmcp` (see above; revisit if Kepler needs resources, streaming, auth or server-initiated requests), hand-rolled
tokio server (needless async for a request/response stdio loop).
