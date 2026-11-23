# Performance notes (Deliveries 0-2)

## Measured results (single machine, one run each; not a benchmark suite)
Environment: Windows 11 (10.0.26200), Intel Core i5-7300U @ 2.60 GHz, `cargo build --release` (thin LTO, stripped), rustc 1.98.0,
dependency versions as in `Cargo.lock`, 2026-11-23. Database seeded with 500 short memories. Numbers are what the commands below
printed; they will differ on other hardware. No p99 claim beyond n=2000-3000 samples.

**Daemon resources** (`kepler daemon --idle-timeout-secs 0`, release, Forge + Memory + Guardian enabled):

| Measure | Result |
|---|---|
| Binary size (`kepler.exe`) | 3.81 MB |
| Idle working set (10 s after start) | 7.6 MB (target in OVERVIEW: < 30 MB: met on this machine) |
| Working set after first memory retrieval / after 100 more events | 8.3 MB / 8.5 MB |
| Idle CPU over 20 s | 0.0 ms (no polling) |
| Threads at idle | 5 (runtime thread, storage writer, OS/runtime helpers) |
| One-shot `kepler emit` incl. process start, daemon already running | about 35 ms each (100 runs; dominated by process creation on Windows) |

**Dispatch latency** (`cargo run --release -p kepler-core --example bench`; microseconds):

| Path | p50 | p95 | p99 |
|---|---|---|---|
| in-process: trivial input (`ok`), no module runs | 0.4 | 0.8 | 0.9 |
| in-process: tool preflight (clean command) | 2.6 | 6.9 | 7.4 |
| in-process: tool result scan, 4 KiB clean excerpt | 170 | 379 | 477 |
| in-process: specific input (Forge passthrough + Memory FTS over 500 rows) | 481 | 821 | 986 |
| in-process: vague input (Forge enrich + Memory + Guardian budget) | 618 | 1020 | 1242 |
| IPC round trip, tool preflight (open connection, real daemon, named pipe) | 66 | 87 | 147 |
| IPC round trip, specific input | 420 | 920 | 1070 |
| IPC round trip, vague input (enrich) | 538 | 1131 | 1314 |

Reading: simple dispatch is far below the 10 ms target; a warm memory retrieval costs about 0.5 ms here (target < 100 ms), but it
runs synchronously on the daemon's single runtime thread, so a slow disk or a much larger database delays other connections (not measured:
databases beyond 500 memories, cold cache, concurrent clients with retrieval). The result scan is the most expensive security check (redaction
is a linear scan; 16 KiB excerpts cost roughly 4x the 4 KiB figure, not measured). `max` outliers up to 18 ms were observed once (first call
after warm-up). Not measured: CPU/RSS of the Pi adapter, Node startup, end-to-end latency inside Pi, Linux/macOS.

Reproduce: `cargo build --release -p kepler-cli && cargo run --release -p kepler-core --example bench`; for RSS start
`target/release/kepler daemon --idle-timeout-secs 0` with `KEPLER_HOME` and `KEPLER_IPC_NAME` pointing at scratch values and sample
the process working set (Task Manager or `Get-Process`).

## Design choices that bound cost (and anticipated costs not yet measured)
Everything below the measured tables above is design rationale.
- Daemon uses a Tokio **current-thread** runtime: one OS thread for all I/O plus one storage writer thread (blocked on a channel, no timers).
- **No polling anywhere:** the accept loop waits on I/O; idle shutdown is a single timer armed only while there are zero connections.
- Lazy start: nothing runs until the first `kepler emit`; default idle shutdown 300 s (`--idle-timeout-secs`, `KEPLER_IDLE_TIMEOUT_SECS`).
- Bounded everything: 32 connections, 1 in-flight request per connection, 128 KiB frames, 64 KiB envelopes, storage queue of 256 (drops, never blocks), 16 sessions per connection / 1024 total, <=16 policy modules, 10 000 stored event rows.
- Hot path is allocation-light but not allocation-free: one JSON parse, a `Value` -> typed envelope conversion, string checks, rule matching, one `String` per reply.
- Persistence is metadata only and batched into short transactions (up to 64 commands).
- No LLM, network, browser or embedding is ever started (Deliveries 0-2). Delivery 1-2 added per-event module work (Forge scan, one FTS5 query, Guardian budget) bounded by a 100 ms module budget and 16 KiB output caps.

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
