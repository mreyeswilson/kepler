# Getting started (planned workflow)

## Current status
The Rust workspace under `crates/` implements Deliveries 0-5 (events, policy, storage, IPC, daemon, Forge, Memory, Guardian, Focus, Sentinel, Vision manager, Orbit, MCP server, CLI) with tests, and `adapters/pi` implements the automatic hooks. Nothing is verified against a real Pi session, a real browser or non-Windows systems, and there is no installer or published binary. See the README status section, the [backlog](../roadmap/BACKLOG.md) and the [performance notes](PERFORMANCE.md).

## Prerequisites
- Rust stable toolchain (>= 1.89, see ADR-0019) using rustup
- Git and Cargo
- Node.js >= 22.18 for `adapters/pi` and `workers/vision`; Git (Sentinel and Orbit tests use real repositories)
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

## Vision worker checks

From `workers/vision`: `npm ci --ignore-scripts && npm run typecheck && npm run build && npm test`. No browser is downloaded by these steps; a real-browser run needs `npx playwright install chromium` and `KEPLER_VISION_WORKER` (see `workers/vision/README.md`).

## Development quality
- Run `cargo fmt --check`, `cargo clippy --all-targets --all-features`, `cargo test --workspace` when project code exists.
- Use representative Linux/macOS/Windows fixtures.
- Never enable cross-provider network calls by default.
- Record benchmark environment and exact dependency versions.

## First milestone
Demonstrate that a Pi user instruction emits a normalized event, Kepler receives it, applies deterministic passthrough and returns without blocking ordinary Pi execution.
