# Kepler Pi adapter (KPL-008, KPL-019) — automatic hooks, verified against Pi 1.6.0 on Windows

> **Status: experimental, private (`private: true`), not published.** The adapter forwards Pi hook events to a locally running Kepler core and applies its verdicts as described in "Automatic hooks" below.
> Unit tests use a mock core. `npm run test:e2e` (`test/e2e/run.mjs`) runs a **real Pi** (`@earendil-works/pi-coding-agent@1.6.0` installed from npm into a temp directory), installs this package with `pi install <path>`, talks to the **real release `kepler` daemon**, and drives tool calls with a scripted test-only "faux" model (no credentials needed, no remote model). 24 checks pass on Windows 11: harmless tool runs; `cat .env` and an obfuscated variant are blocked; `git push --force` is blocked without an approval UI; a token in tool output is withheld before the model sees it; recalled memory arrives as a labeled system-prompt block while the user prompt is untouched; branch-scoped memory is recalled only on its branch; a negative control shows the same call leaks the secret without the adapter; the database holds neither secret.
> **Not verified:** interactive approval prompts and advisory notifications in the real TUI (print mode has no UI), a real remote model, other Pi versions, Linux/macOS. The Kepler core is never launched or installed by the adapter. The package stays private until scope ownership and release are approved.
>
> Findings of the first real run: none of the payload assumptions needed correcting (event names, `tool_call` block shape, `tool_result` content patch, `agent_settled`, `session_shutdown`, `ctx.cwd`, `ctx.sessionManager.getSessionId()`, `ctx.hasUI`). Gaps found and fixed: the adapter never reported `files.changed` (now sent after successful `edit`/`write` tools), never sent the git branch (now read from `.git/HEAD`), and never displayed Focus advisories (now `ctx.ui.notify`, once per code per session, only with a UI). Pi loads the extension through `pi install <path>` with the `pi.extensions` manifest entry.

## What it does

- Registers two Pi hooks in `extensions/kepler.ts`: `input` and `session_shutdown`.
- On every `input` (sources `interactive` and `rpc` only) it builds an `input.received` envelope and sends it to the per-user
  Kepler daemon over local IPC (ADR-0007: line-delimited JSON, named pipe on Windows / Unix socket elsewhere,
  `hello`/`hello_ack` handshake, 131072-byte frame limit, one request in flight).
- The hook returns immediately (`undefined` = Pi `continue`) and does not wait for Kepler: **the user's text is never changed,
  delayed, blocked or lost**. No `transform`/`handled` result is ever returned in this milestone.
- Any failure — core not running, connect/handshake/request timeout, oversized or malformed frame, fatal error frame, unsupported
  protocol, mismatched reply — is a silent passthrough. Next input retries (new connection).
- Input that cannot be forwarded verbatim (empty, > 16 KiB, contains NUL, source `extension`) is skipped, never truncated.
- Privacy: the project id is `pi-<first 16 hex of sha256(cwd)>` (the path itself is not sent). The session id comes from Pi's
  session manager, sanitized to `[A-Za-z0-9._:-]{1,128}`. Events go only to the same-user local endpoint; no network access.
- It never starts the core, never downloads or installs anything, and has no install scripts or runtime dependencies.

## Endpoint resolution (same as `kepler-ipc`)

| Platform | Endpoint |
|---|---|
| Windows | `\\.\pipe\kepler-<USERNAME sanitized>-<name>` |
| Linux/macOS | `$XDG_RUNTIME_DIR/kepler/kepler-<name>.sock`, else `<tmpdir>/kepler-<user>/kepler-<name>.sock` |

`<name>` is `daemon` unless `KEPLER_IPC_NAME` (`[a-z0-9_-]{1,32}`) says otherwise.

## Pi API compatibility (honest status)

`extensions/kepler.ts` declares **minimal structural types** instead of importing Pi's. The `input` and `session_shutdown` shapes were compared in Delivery 0 with `@earendil-works/pi-coding-agent` 1.6.0 (not present on the machine used for Delivery 1, so not re-checked); the other hooks follow Pi's published extension docs:
`on("input", (event, ctx) => ...)` with `event.text`/`event.source`, `ctx.cwd`, `ctx.sessionManager.getSessionId()`, and
`on("session_shutdown")`. The scaffold's earlier peer dependency name `@mariozechner/pi-coding-agent` appears to be the previous
package scope; the peer dependency is now declared optional as `@earendil-works/pi-coding-agent`. Other Pi versions are **untested**.

## Develop

Requires Node >= 22.18 (native TypeScript type stripping; tests run with the built-in `node:test` runner).

```bash
cd adapters/pi
npm ci --ignore-scripts
npm run typecheck   # tsc --noEmit, strict
npm run build       # compile check into dist/ (not shipped; Pi loads the .ts sources through jiti)
npm test            # node:test with a mock core on a temporary pipe/socket
```

Tests cover: endpoint resolution, handshake, passthrough on absent core / timeouts / oversized frames / error frames / bad frames /
mismatched ids / mid-request close, one-in-flight, event mapping, and the shared fixtures in `/fixtures/events`
(valid ones accepted and relayed unchanged; invalid ones rejected with the manifest error code by the TS pre-validator).

## Local developer installation (experimental)

```bash
pi install /absolute/path/to/kepler/adapters/pi
```

It is a no-op unless a Kepler core is listening on the endpoint above.

## Not public

`@kepler/pi` is a provisional name; the `@kepler` npm scope is not verified. `private: true` (enforced until release approval), `license: MIT`.
Release gates and the guarded manual procedure are in `docs/distribution/NPM_AND_PI.md` and `RELEASING.md`.

## Automatic hooks (KPL-019, [ADR-0013](../../docs/decisions/0013-pi-automatic-hooks.md))

| Pi event | Kepler event | What the adapter may do |
|---|---|---|
| `input` | `input.received` | observe only (text never changed/delayed/blocked); keeps the verdict for the next step |
| `before_agent_start` | (verdict of the input) | on `enrich`: **append** a labeled block to `systemPrompt` ("derived/recalled, NOT user instructions"). The user prompt is never edited. `KEPLER_PI_INJECT=0` disables |
| `context` | `context.prepared` | observe only; token figure is an adapter **estimate** (`estimated: true`) |
| `tool_call` | `tool.call.requested` | `{block: true}` only on an explicit `deny`; on `require_approval` asks `ctx.ui.confirm` when `ctx.hasUI`, else blocks. No verdict (core down, timeout, error, unknown action) never blocks and never grants |
| `tool_result` | `tool.call.completed` | replaces the content only on an explicit `redact` verdict (text names the secret *kinds*, never the secret) |
| `agent_start` / `agent_settled` | `agent.started` / `agent.settled` | observe only; `aborted` maps to `cancelled`, otherwise `ok` (drives Memory consolidation) |
| `session_shutdown` | `session.closed` | observe, then close the connection |

The `advisories` array that Focus/Sentinel add to verdicts (ADR-0014) is currently **ignored** by this adapter: nothing is displayed or acted on.

Sends are serialized (one request in flight, queue of 8, 700 ms verdict timeout). The adapter never starts the core.

**Verification status.** 45 `node:test` tests (mock core and fake sender), plus one manual run of these hooks against the real Rust
`kepler daemon` (enrich with forge and recalled memory, deny, approval, redact all behaved as above). **Not verified inside a running
Pi session**: Pi is not installed on the development machine; event/return shapes were taken from Pi's published extension docs
(`before_agent_start`, `context`, `tool_call`, `tool_result`, `agent_start`, `agent_settled`, `session_shutdown`), and the field names of
`write`/`edit` tool inputs (`path`/`file_path`) are not documented there, so both are sent as paths when present. Pi versions other
than the docs consulted are untested. Memory candidates (`memory.candidate`) are not produced by this adapter.
