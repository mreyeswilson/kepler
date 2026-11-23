# ADR-0005: IPC wire protocol

**Status:** Accepted (implemented in `kepler-ipc`, Delivery 0)

## Context
Harness adapters (TypeScript for Pi, other languages later) must send events to a per-user Rust
daemon and get a decision back, without a heavyweight RPC stack, with strict resource bounds and
without exposing the daemon to other local users. A missing/slow daemon must never block the harness.

## Decision

### Transport and isolation
| OS | Endpoint | Isolation |
|---|---|---|
| Linux / macOS | Unix domain socket `<runtime-dir>/kepler-<name>.sock`; `<runtime-dir>` is `$XDG_RUNTIME_DIR/kepler` or, if unset, `<tmp>/kepler-<user>` | directory `0700`, owned by the current user (verified, otherwise refuse to start); socket `0600` |
| Windows | Named pipe `\\.\pipe\kepler-<user>-<name>` | DACL created at pipe creation: protected, one ACE granting `GENERIC_ALL` to the **current user's SID** only; remote clients rejected; `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance to prevent pipe squatting |

`<name>` defaults to `daemon` (`KEPLER_IPC_NAME` overrides it; `[a-z0-9_-]{1,32}`), `<user>` is the sanitized OS user name.
Isolation is by OS user only: any process running as the same user is trusted at the transport level, so
events are still validated as untrusted input.

### Framing
UTF-8 JSON, one frame per line, terminated by `\n` (a trailing `\r` is tolerated). Max frame size
**131072 bytes** (excluding newline); the max event envelope is 65536 bytes. A frame over the limit
is answered with a fatal `frame_too_large` error and the connection is closed (the reader never buffers
more than the limit). Blank lines count as errors.

### Handshake
1. Client sends `{"type":"hello","protocol":1,"client":"pi-adapter","client_version":"0.1.0","capabilities":{...}}`
   (`client_version` and `capabilities` optional; `capabilities` is a `kepler-events` advertisement).
2. Server answers `{"type":"hello_ack","protocol":1,"server_version":"..","max_frame_bytes":131072,"negotiated":{...}|null}`.
   Unsupported `protocol` -> fatal `{"type":"error","code":"unsupported_protocol",...}` and close.
3. Any other first frame -> fatal `handshake_required`.

### Requests (after handshake)
| Client frame | Server reply |
|---|---|
| `{"type":"event","request_id":"r1","event":{<envelope v1>}}` | `{"type":"decision","request_id":"r1","event_id":"..","decision":{"action":"passthrough","reason":".."}}` |
| `{"type":"ping","request_id":"p1"}` | `{"type":"pong","request_id":"p1"}` |

Decision `action`: `passthrough`, `enrich` (with `derived[]`, always labeled `inferred`, never replacing the original),
`require_approval`, `deny`. An optional `degraded[]` lists swallowed optional-module failures.
Replies are strictly ordered with requests on a connection (one in flight at a time).

### Errors
`{"type":"error","request_id":null|"..","code":"..","message":"..","fatal":bool}`. Codes: `bad_frame`,
`empty_frame`, `frame_too_large` (fatal), `handshake_required` (fatal), `unsupported_protocol` (fatal),
`invalid_capabilities` (fatal), `too_many_errors` (fatal), `bad_request`, event validation codes
(`malformed`, `unsupported_version`, `unknown_kind`, `invalid_field`, `field_too_large`, `oversized`),
`session_conflict`, `session_limit`, `handler_timeout`, `internal`. Non-fatal errors keep the connection open;
after 8 consecutive errors the server closes it.

### Limits and timeouts (defaults)
Handshake 2 s; idle read 60 s; write 5 s; handler budget 500 ms; max 32 concurrent connections (extra
connections are closed immediately, no queue); one in-flight request per connection; daemon idle shutdown
300 s (CLI default) with no active connection.

### Adapter fallback contract
Connection failure, handshake failure, timeout, `handler_timeout`, EOF or any error means **passthrough** of the
original input. For `tool.call.requested`, absence of a verdict never *grants* anything: the harness's own permission
flow stays in force; only an explicit `deny`/`require_approval` from Kepler restricts further.

### Session identity
`session_id` is validated (`[A-Za-z0-9._:-]{1,128}`). The daemon binds each session id to the first connection that uses
it; another live connection presenting the same id gets `session_conflict`. A connection may bind at most 16 sessions;
the daemon tracks at most 1024. Bindings are released when the connection closes or on `session.closed`.
This is not authentication (same-user processes are trusted); it prevents accidental/forged cross-talk between live sessions.

## Consequences
Trivial to implement in any language (line-delimited JSON), debuggable with a terminal, bounded memory.
No multiplexing and no streaming in v1; a future protocol version can add them behind the `hello.protocol` field.
The Windows ACL requires a small, documented `unsafe` FFI module (no extra crates). Unix isolation relies on
directory ownership/permissions and could not be exercised on Windows CI hosts; the Unix path is covered by the CI matrix.

## Alternatives
gRPC/tonic (large dependency and binary size), HTTP on loopback (reachable by every local user/browser, port management),
length-prefixed binary framing (harder to debug from adapters), stdio-only (no shared daemon).
