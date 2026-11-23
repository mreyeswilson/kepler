# Kepler Pi adapter (KPL-008) — observe-only, unverified against real Pi

> **Status: experimental, private, not published.** The adapter forwards each user `input` to a locally running Kepler core as an
> `input.received` v1 event and does nothing else. It is tested against a mock core (tests in this directory) and was checked once,
> manually on Windows, against the real Rust daemon (`kepler daemon`: handshake, passthrough decision, `invalid_field` error).
> It has **not** been tested end to end inside a running Pi session, and the Kepler core is not launched or installed by it.
> Do not describe it as working with Pi until an e2e test with real Pi exists.

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

`extensions/kepler.ts` declares **minimal structural types** instead of importing Pi's. They were compared manually (and with a
scratch `tsc` assignability check, not committed) against the locally installed `@earendil-works/pi-coding-agent` 1.6.0:
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

`@kepler/pi` is a provisional name; the `@kepler` npm scope is not verified. `private: true`, `license: UNLICENSED`.
Release gates and the guarded manual procedure are in `docs/distribution/NPM_AND_PI.md` and `RELEASING.md`.
