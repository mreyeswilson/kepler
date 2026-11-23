# ADR-0006: Runtime dependencies for Delivery 0 (SQLite binding and friends)

**Status:** Accepted (implemented in Delivery 0)

## Context
AGENTS.md requires an explicit ADR for new runtime dependencies. ADR-0003 already chose SQLite + WAL + FTS5;
this ADR records the crates used to realize it and the rest of the Delivery 0 dependency set.

## Decision
| Crate | Used by | Why | Cost / notes |
|---|---|---|---|
| `rusqlite` (feature `bundled`) | `kepler-storage` | Thin, synchronous SQLite binding. `bundled` compiles SQLite with **FTS5 enabled**, so behavior and FTS5 availability do not depend on the system library (Windows/macOS/Linux parity, reproducible tests). | Needs a C compiler at build time; adds a few hundred KB to the binary. No runtime service, no network. |
| `tokio` (minimal features: `rt`, `net`, `io-util`, `time`, `sync`, `macros`) | `kepler-ipc`, `kepler-core`, `kepler-cli` | Required by ADR-0001. Current-thread runtime in the daemon keeps idle RAM low; named pipes and Unix sockets come from the same crate. | `rt-multi-thread` is only a dev-dependency (tests). |
| `serde`, `serde_json` | events, policy, ipc, core, cli | Strict typed (de)serialization with `deny_unknown_fields`; wire format is JSON. | No `serde_json` `preserve_order`/`arbitrary_precision`. |
| `clap` (derive, no color/suggestions) | `kepler-cli` | Standard CLI parsing with `env` support for daemon options. | Default features disabled to keep size small. |

Not added on purpose: `chrono`/`time` (RFC 3339 validated by a small strict parser; SQLite supplies timestamps),
`regex` (redaction is hand-written, linear time, no ReDoS), `dirs` (platform directories are 25 lines),
`windows-sys`/`libc` (the only FFI is a small documented module for the pipe ACL), `async-trait`, `thiserror`, `anyhow`, `tracing`.

SQLite usage rules: WAL, `synchronous=NORMAL`, `busy_timeout=2s`, `foreign_keys=ON`, `trusted_schema=OFF`; exactly one writer thread
(single write coordinator) behind a bounded queue (256) that **drops** instead of blocking; read-only connections for queries;
versioned, append-only migrations in transactions, databases from a newer version are refused.

## Consequences
Bundled SQLite means security updates arrive with `rusqlite` updates (track via dependabot/`cargo audit` in a later delivery).
The database is **not encrypted** (see threat model); privacy relies on not storing raw prompts/tool output and on redaction.

## Alternatives
`sqlx` (async, heavier compile and macro cost, no benefit for one writer thread), `libsql`/`turso` (extra runtime, remote features not wanted),
system SQLite (FTS5 not guaranteed, version drift), `sled`/files (no FTS5, no SQL migrations).
