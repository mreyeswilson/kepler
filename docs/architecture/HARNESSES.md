# Harness integration strategy

## Levels
1. **CLI only:** any environment capable of running commands.
2. **MCP:** callable tools exposed by `kepler mcp serve`.
3. **Native extension:** events, injection, tool preflight and lifecycle automation when supported.

## Initial targets
- **Pi Coding Agent:** TypeScript native extension; initial full integration test target.
- **Claude Code:** native hooks/plugin + MCP, subject to current API.
- **Gemini CLI:** extension/hooks + MCP.
- **OpenCode:** plugin/event integration + MCP.
- **Codex:** skills/plugins, MCP and hooks where available; validate exact version/capabilities.

The matrices and interfaces are **plans**, not guarantees. Revalidate official APIs before each adapter implementation.

## Adapter responsibilities
- Detect installed harness/version.
- Negotiate capabilities.
- Map native payloads to versioned Kepler events.
- Preserve original user instruction and trust labels.
- Connect to per-user daemon using least-privilege IPC.
- Keep network and heavy logic in core or dedicated worker.
- Degrade explicitly for absent capabilities.

## Pi initial contract
Investigate `input`, `before_agent_start`, `context`, `tool_call`, `tool_result`, `agent_settled` event semantics and their version constraints. Add typed fixtures and end-to-end tests before enabling automatic transformations.

## Adapter test matrix
For each version, test: installation, event delivery, input passthrough, context injection, tool preflight (if supported), session recovery, uninstall and missing-daemon fallback.

## Pi distribution
See [npm and Pi distribution](../distribution/NPM_AND_PI.md). `adapters/pi` is an **experimental, observe-only** adapter (forwards `input` as `input.received` over IPC; never transforms or blocks) tested only against a mock core, **not verified with a real Pi session**. It must not be marketed as an operational extension. Public npm publication stays disabled until all release gates pass.

## Claude Code
Implemented as `kepler hook claude-code <Event>` (Rust, no Node) plus MCP; see [ADR-0020](../decisions/0020-claude-code-hooks-adapter.md) and the README section "Conectar con Claude Code". Capabilities: `lifecycle_observe`, `tool_preflight` (deny/ask only), `tool_result_observe` (observe; no mutation) and `context_inject` (labeled `additionalContext`). `input_transform` and result redaction are not provided. Verified only with Claude Code 2.1.330 on Windows.

## Desktop clients and discovery

Kepler will detect desktop apps as well as terminal harnesses: **Claude Desktop**, Cursor, VS Code/GitHub Copilot, Windsurf and generic MCP clients, alongside the coding agents above. Discovery is not yet implemented. See [discovery and installation architecture](DISCOVERY.md).

Claude Desktop chat will initially receive **MCP tool access**, potentially via an `.mcpb` Desktop Extension; this does **not** imply native prompt interception. Claude Code (including supported Code sessions inside Desktop) is a separate integration surface with additional hooks. Capability negotiation determines which automatic modules can actually run.
