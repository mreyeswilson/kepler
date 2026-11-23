# ADR-0024: Genesis local UI (`kepler ui`)

Status: accepted. Implements the "local UI" part of ADR-0018 only; design generation, previews
and code generation (KPL-034..036) remain unimplemented.

## Decision
`kepler ui [--port N] [--allow-actions] [--open] [--json]` starts an HTTP server in the
`kepler-ui` crate and prints `http://127.0.0.1:<port>/?t=<token>`. It is a foreground process
(Ctrl+C stops it), not a daemon.

Security properties, each covered by `crates/kepler-ui/tests/security.rs` over real sockets:
* binds `127.0.0.1` only, random port by default, connection cap, one request per connection;
* a 256-bit session token from the OS CSPRNG (`BCryptGenRandom` on Windows, `/dev/urandom`
  elsewhere: one small `unsafe` FFI call, like the named-pipe ACL module; no new dependency)
  is required on every request. The landing URL `/?t=` sets an `HttpOnly; SameSite=Strict`
  cookie and redirects to `/` so the token leaves the URL; `Authorization: Bearer` also works.
  Missing/wrong token -> 401. The query token is accepted only on `/`;
* the `Host` header must be exactly `127.0.0.1:<port>` or `localhost:<port>` (anti
  DNS-rebinding, checked before authentication -> 421); `Origin` and `Sec-Fetch-Site`, when
  present, must be same-origin (-> 403);
* API is read-only: overview, events, metrics, memories, findings, Orbit runs. Nothing returns
  prompts or tool output (none are stored); memory text is redacted again on output;
* the only write is `POST /api/orbit/resolve {run, task, grant, confirm}`: it exists only with
  `--allow-actions`, needs `application/json`, the same origin, and `confirm` equal to the task
  id (the page asks the user to type it). It only flips the same database row as
  `kepler orbit approve|deny`; `kepler orbit resume` still runs the work;
* strict CSP (`script-src 'self'`, no inline script, `frame-ancestors 'none'`), `nosniff`,
  `no-store`, `no-referrer`; the page builds all dynamic content with `textContent` (verified in
  a real browser with an `<img onerror>` payload stored as memory text);
* request limits: 16 KiB headers, 64 headers, 4 KiB body, no chunked bodies, 5 s read timeout.

## Not done / limits
* No TLS (loopback only), no multi-user model: any process of the same OS user can read the
  token from the command line output or process memory, exactly like it can read the SQLite file.
* It is a dashboard: no Genesis design workflow, no editing of memories or findings (use the CLI).
* Verified on Windows only (including a real browser pass); the Unix branch of the RNG is
  untested here.
