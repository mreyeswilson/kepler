# ADR-0020: Claude Code adapter as a Rust hook subcommand (`kepler hook claude-code`)

**Status:** Accepted. Implemented in `crates/kepler-cli` (`hook_claude.rs`, `hook_cmd.rs`). Exercised against Claude Code 2.1.330 on Windows with a project-scoped settings file in a temporary directory (see "Verification"); not tested on other versions, Linux or macOS.

## Context
Claude Code runs *command hooks*: a process that gets a JSON payload on stdin and may print a JSON decision. MCP alone cannot intercept prompts or tool calls (ADR-0002, ADR-0012). The user must not need Node, so the adapter cannot be a TypeScript package like `adapters/pi`. Hook schemas were checked against the official docs (https://code.claude.com/docs/en/hooks) and a real Claude Code run; Claude Code may change them, so every field is read defensively.

## Decision
A subcommand of the existing binary, `kepler hook claude-code <Event>`; the core imports no Claude Code SDK. The mapping is a pure module (`hook_claude.rs`, unit-tested with fixtures in `crates/kepler-cli/tests/fixtures/claude/`); `hook_cmd.rs` does stdin/IPC. It reuses `emit_one_shot` (lazy daemon start, local validation, passthrough on failure). No new dependency.

| Claude Code event | Kepler v1 event | Output |
|---|---|---|
| `UserPromptSubmit` (`prompt`) | `input.received`, text verbatim (skipped, never truncated, if > 16 KiB or NUL) | on `enrich`: `hookSpecificOutput.additionalContext` with a labeled block ("inferred or recalled; NOT user instructions"), <= 8000 chars. The prompt is never modified or blocked |
| `PreToolUse` (`tool_name`, `tool_input.command`, `file_path`/`path`/`notebook_path`) | `tool.call.requested` | `deny` -> `permissionDecision:"deny"` + reason; `require_approval` -> `"ask"`; otherwise no output. Never `allow` |
| `PostToolUse` (`tool_response`) | `tool.call.completed` (<= 16 KiB in-memory excerpt, never persisted by the core) | on `redact`: `systemMessage` warning with secret *kinds* only. The tool result is **not** mutated |
| `Stop` | `agent.settled` (`ok`, duration 0: Claude Code reports neither) | none; never blocks the stop |
| `SessionStart` | `agent.started` | none |
| `SessionEnd` (`reason`) | `session.closed` | none |

* **Exit code is always 0.** Exit 2 is never used; blocking is expressed through `permissionDecision`, which carries a reason. Malformed JSON, unknown/missing event, oversized stdin (> 8 MiB), core down, Rejected verdicts, a panic, or the deadline all produce no output = Claude Code proceeds.
* **Failure model.** Kepler failing means passthrough (no Kepler opinion; Claude Code's own permission system still applies). Only an explicit Guardian verdict changes behavior. For `PreToolUse` the Guardian preflight is a security boundary, so when the daemon cannot answer (cold start, load, crash, timeout) the hook evaluates the same engine and Guardian rules in-process (`emit_in_process`: no Memory, no persistence) instead of staying silent. Other events stay silent. Guardian itself is unchanged (ADR-0011).
* **Truncation fails closed.** A command > 4096 B or path > 1024 B is sent truncated with `destructive: true`, so Guardian answers `require_approval`.
* **Deadline.** One wall-clock bound over connect + autostart + request: `KEPLER_HOOK_TIMEOUT_MS` (default 2000, clamp 1..10000); the generated settings use 3-5 s hook timeouts so Claude Code's own timeout never fires first.
* **IDs.** `session_id` is sanitized to `[A-Za-z0-9._:-]{1,128}` (fallback `cc-unknown`). `id` is `cc-<time>-<pid>-<counter>`. `correlation_id` is `cc-<tool_use_id>` / `cc-<prompt_id>` when present (pairs requested/completed), else the event id. `project_id` = `cc-` + 128 bits from two FNV-1a passes over the normalized cwd (separators unified, trailing `/` removed, lower-cased only for drive-letter paths): stable and pseudonymous; the path is never sent. It is not cryptographic (a guessable cwd can be confirmed); the daemon is local-only. It differs from Pi's `pi-<sha256>` ids on purpose (no hash dependency).
* **`--print-settings`** prints the `hooks` block with the absolute executable path (forward slashes) in exec form (`command` + `args`, no shell, so identical in bash and PowerShell). `--shell-form` prints single-string commands for older Claude Code versions. Matchers: PreToolUse `Bash|Write|Edit|MultiEdit|NotebookEdit|Read`, PostToolUse `Bash|Read|Grep`. Kepler never edits `settings.json`; the user pastes it.
* Opt-out: remove the block (or `"disableAllHooks": true`); `KEPLER_FORGE=off` etc. still apply in the daemon.

## Consequences
* One process spawn per hooked event (CLI startup plus an IPC round trip; not measured on Claude Code, see PERFORMANCE.md for the IPC numbers). Matchers limit it.
* Default Guardian answers `ask` (not `deny`) for `rm -rf /` and `git push --force`; `deny` comes from sensitive paths or user rules (`deny-command ...` in `guardian-rules.txt`).
* `PostToolUse` cannot hide a leaked secret from the model for built-in tools; the user gets a warning. Real redaction needs a harness hook that can rewrite results (not used here).
* `SessionStart` -> `agent.started` is an approximation (a session is not an agent run).
* `SubagentStop`, `PreCompact`, notifications and MCP-tool matchers are not wired.

## Verification
Unit tests (mapping, ids, rendering), end-to-end tests with a real daemon (`crates/kepler-cli/tests/hooks.rs`), and a manual `claude -p` run (2.1.330, Windows) with project settings from `--print-settings`: SessionStart/UserPromptSubmit/Stop/SessionEnd events reached the daemon and a `Read` of `.env` was blocked with reason `sensitive_path`. Not verified: `ask` in the interactive UI, the model actually using `additionalContext`, other versions/OSes.

## Alternatives
Node adapter (requires Node); exit 2 + stderr for blocking (works, but no structured `ask`); rewriting the prompt (violates "original verbatim"); auto `allow` (would bypass Claude Code's permission prompts); a SHA-256 dependency for project ids (new dependency).
