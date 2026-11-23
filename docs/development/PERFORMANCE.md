# Performance notes (Delivery 0)

**Nothing here is a benchmark result.** These are design choices that should keep costs low and
a procedure to measure them. The targets in the architecture overview remain targets.

## Design choices that bound cost
- Daemon uses a Tokio **current-thread** runtime: one OS thread for all I/O plus one storage writer thread (blocked on a channel, no timers).
- **No polling anywhere:** the accept loop waits on I/O; idle shutdown is a single timer armed only while there are zero connections.
- Lazy start: nothing runs until the first `kepler emit`; default idle shutdown 300 s (`--idle-timeout-secs`, `KEPLER_IDLE_TIMEOUT_SECS`).
- Bounded everything: 32 connections, 1 in-flight request per connection, 128 KiB frames, 64 KiB envelopes, storage queue of 256 (drops, never blocks), 16 sessions per connection / 1024 total, <=16 policy modules, 10 000 stored event rows.
- Hot path is allocation-light but not allocation-free: one JSON parse, a `Value` -> typed envelope conversion, string checks, rule matching, one `String` per reply.
- Persistence is metadata only and batched into short transactions (up to 64 commands).
- No LLM, network, browser or embedding is ever started in Delivery 0.

## Anticipated costs (unmeasured)
- One-shot `kepler emit` with a running daemon: process start + one local round trip. Process start dominates; an adapter that keeps a connection open avoids it.
- Cold `kepler emit` with autostart: spawns the daemon and retries the connection with 20 ms -> 250 ms backoff (up to 3 s). The harness is never blocked longer than `EmitOptions::timeout` + `autostart_wait` and always gets a passthrough on failure.
- Bundled SQLite adds binary size; the database file is created on first daemon start.

## How to measure
1. `cargo build --release`; run `kepler daemon --idle-timeout-secs 0`.
2. Idle RSS/CPU: sample the process (Task Manager / `ps -o rss,pcpu`) for 60 s with no clients.
3. Event latency: keep one connection open (see ADR-0007) and send N events; record p50/p95/p99 per request. Compare with `kepler emit --in-process` to separate process start from dispatch.
4. Flood behavior: the `event_flood_*` and `connection_flood_*` tests show correctness under load, not throughput.
Record OS, CPU, build profile and exact dependency versions (`Cargo.lock`) with any result.
