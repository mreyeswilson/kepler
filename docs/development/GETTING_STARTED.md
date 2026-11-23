# Getting started (planned workflow)

## Current status
A Pi npm **package scaffold** now exists under `adapters/pi`, but no functional Kepler CLI, hook integration or Rust source exists yet. This document describes the intended development environment, not a working installer.

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

## Pi package scaffold checks

From `adapters/pi`, run `npm pack --dry-run --json` to inspect the package contents. The manifest is intentionally marked private; do not publish it. See [distribution guide](../distribution/NPM_AND_PI.md).

## Development quality
- Run `cargo fmt --check`, `cargo clippy --all-targets --all-features`, `cargo test --workspace` when project code exists.
- Use representative Linux/macOS/Windows fixtures.
- Never enable cross-provider network calls by default.
- Record benchmark environment and exact dependency versions.

## First milestone
Demonstrate that a Pi user instruction emits a normalized event, Kepler receives it, applies deterministic passthrough and returns without blocking ordinary Pi execution.
