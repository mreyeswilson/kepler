# Getting started (planned workflow)

## Current status
An experimental, observe-only **Pi adapter** exists under `adapters/pi` (IPC client + `input` hook, mock-core tests only, not verified with real Pi). The Rust workspace under `crates/` implements Delivery 0 (events, capabilities, policy, IPC, storage, daemon, `kepler doctor|daemon|emit|version`) with tests; there is **no hook integration, no installer and no published binary**. See the README status section and the [performance notes](PERFORMANCE.md).

## Prerequisites
- Rust stable toolchain using rustup
- Git and Cargo
- Node.js for Pi extension and future Playwright worker
- Pi Coding Agent for initial adapter tests

## Bootstrap sequence
1. Create Cargo workspace for core/events/policy/CLI/storage/IPC.
2. Add tests and CI before enabling automatic hooks.
3. Implement first event receiver and a local passthrough test.
4. Create the Pi extension to forward `input` events.
5. Add privacy filters, SQLite and memory.
6. Benchmark idle and event performance.

## Pi adapter checks

From `adapters/pi` (Node >= 22.18): `npm ci --ignore-scripts && npm run typecheck && npm run build && npm test`, then `npm pack --dry-run --json` to inspect the package contents. The manifest is intentionally marked private; do not publish it. See [distribution guide](../distribution/NPM_AND_PI.md).

## Development quality
- Run `cargo fmt --check`, `cargo clippy --all-targets --all-features`, `cargo test --workspace` when project code exists.
- Use representative Linux/macOS/Windows fixtures.
- Never enable cross-provider network calls by default.
- Record benchmark environment and exact dependency versions.

## First milestone
Demonstrate that a Pi user instruction emits a normalized event, Kepler receives it, applies deterministic passthrough and returns without blocking ordinary Pi execution.
