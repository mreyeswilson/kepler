# ADR-0030: MCP tools for Orbit and the other modules (Orbit "in the flow")

**Status:** Accepted (implemented in `kepler-mcp`, `kepler-cli`). Amends ADR-0012 ("no write tools") and builds on ADR-0017.
The number 0030 leaves room for ADRs written in parallel branches; renumber on merge if needed.

## Context
Orbit existed only as `kepler orbit run plan.json`. Users expected Claude Code/Claude Desktop to be able to split a big
task and delegate the parts to sub-agents. Those clients can only reach Kepler through MCP tools (ADR-0002: MCP cannot
intercept prompts or tool calls). ADR-0012 shipped three read-only tools on purpose, so that a prompt-injected model could
not write or approve anything.

## Decision
1. **More tools, same in-tree server, no new dependency.** The JSON-RPC server stays hand-written on `serde_json`
   (plus the internal `kepler-policy` for redaction). The official Rust SDK is still not worth its dependency graph.
2. **Modular registry.** One file per tool family under `kepler-mcp/src/tools/`, each exporting `tools() -> Vec<Tool>`,
   plus one table (`tools::registry()`). A new module (e.g. Genesis) adds a file and one `extend` line. A `Tool` carries
   name, title, description saying WHEN to use it, a JSON Schema, MCP annotations and an optional handler; the
   operations live behind `Backend::op(name, args)` implemented in `kepler-cli/src/mcp_ops.rs`.
3. **Schema is the validator.** A small validator (type, enum, min/max, length, items, required,
   `additionalProperties: false`, no NUL) applies the very schema advertised in `tools/list`; violations are JSON-RPC
   `-32602`. Numbers outside range are rejected, not clamped.
4. **Annotations** (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) on every tool. Read-only: doctor,
   inspect, memory search/list/export, forge analyze, guardian check, sentinel audit/list, vision status, orbit
   suggest_plan/plan/list/status/logs. With effects (clients should ask): `orbit_run`, `orbit_resume` (start processes),
   `orbit_approve`, `orbit_cancel`, `orbit_cleanup`, `sentinel_check` (runs a project command), `vision_check` (browser),
   `memory_add`; `orbit_deny` is a write but not destructive. Annotations are hints for client UIs, not a boundary.
5. **Orbit tools never block.** `kepler_orbit_run` validates the plan (cycles, limits, secrets, Guardian, and a refusal of
   `--dangerously-skip-permissions` style flags in `agent_args`), creates the run and starts `kepler orbit resume <run>` as
   a detached process (hidden window, std handles non-inheritable, tries to break away from the client's job object),
   returning the run id within about a second. The model polls `kepler_orbit_status`.
6. **Approvals stay human-gated.** `kepler_orbit_approve` and `kepler_orbit_cleanup` require `confirm: true`; the
   descriptions say to set it only after the user's explicit approval in the chat. The server re-runs the Guardian on the
   task command before granting (a denied command cannot be approved). Honest limit: `confirm` is typed by the model, so
   the real guard is the client's per-call confirmation for non-read-only tools plus the user's own review.
7. **Fail-closed switch.** `KEPLER_MCP_READONLY=1` hides and refuses every non-read-only tool.
8. **Delegation = `agent` tasks.** A sub-agent is an Orbit task that runs `claude -p <prompt>` (argv, no shell) in its own
   worktree. `kepler_orbit_suggest_plan` builds the plan deterministically (no model) from subtasks the model chose,
   optionally with a final integration task depending on the leaves, default `--permission-mode acceptEdits`.
9. **Output bounds.** 32 KiB per tool result with a truncation marker; secret redaction on every tool except
   doctor/inspect (whose keys such as `tokens` would be mangled); errors are short (`isError`, <= 800 bytes); logs are
   tail-limited (<= 24 KiB); subprocess-backed tools have wall-clock timeouts and bounded capture.
10. **Memory provenance.** `kepler_memory_add` stores `source = derived` (lower trust, provenance kind `mcp`), never `user`:
    a model must not mint user-trusted memories. User scope is not available through MCP.
11. **Skill/command in `integrations/claude-code/`** teaches Claude when to delegate (rarely), to ask the user first, to
    poll, to summarize and never to approve on its own. Installed by hand; Kepler does not write to `~/.claude`.

## Consequences
* MCP still does not intercept anything; delegation happens only when the model (or user via `/kepler-orbit`) calls the tools.
* A prompt-injected model could call write tools, subject to the client's confirmation prompts and Orbit's own limits
  (4 parallel, timeouts, Guardian, worktree isolation, no push/merge). Hence `KEPLER_MCP_READONLY` and the confirm flags.
* Detached executors may die with the client when its job object forbids breakaway; `kepler_orbit_resume` recovers.
* Verified: unit and stdio e2e tests (fake sub-agent), the official MCP SDK for Node (`@modelcontextprotocol/sdk`
  1.20.0), `claude -p` calling tools through `--mcp-config`, and a real run with two `claude -p` agent tasks plus an
  integration task. Not verified: the Claude Desktop GUI, Linux/macOS.

## Alternatives
Keep MCP read-only and rely on the CLI (no "in the flow"); a blocking `orbit_run` (hits client timeouts); MCP progress
notifications/resources for status (more protocol surface; polling is enough); adopting `rmcp` (see ADR-0012).
