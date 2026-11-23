# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). Nothing has been released: no tags, binaries or npm
packages exist. License: MIT. Everything below is on branch `feat/rust-core-d0` and is
**unreleased** (workspace version `0.0.1`, `publish = false`).

## [Unreleased]

### Added
- **License MIT** (`LICENSE`, 2026, Wilson Mendoza): workspace and package manifests, `deny.toml` and docs updated; the npm release guard no longer blocks on the license (still blocks on `private`, version `0.0.0`, tests, tag and confirmation) and now has tests. Crates stay `publish = false`; `adapters/pi` and `workers/vision` stay `private: true`.
- **Release by tag**: `release-binaries.yml` creates a draft GitHub Release in preflight, uploads each package directly with `gh release upload` (no upload/download-artifact) and un-drafts it after `SHA256SUMS` is added; environment `release`; archives include `LICENSE` and `README.md`.
- **Installers** `install.sh` (Linux/macOS) and `install.ps1` (Windows): HTTPS only, SHA-256 verified against `SHA256SUMS`, no sudo/admin, no telemetry, uninstall; local tests in `tests/installers/` and CI workflow `installers.yml`.
- **Focus** (`kepler-focus`, ADR-0014): non-blocking supervision advisories (scope lock, loop detection, adaptive
  budget, minimal-change guard, evidence/completion gate). Additive `advisories` field on the wire verdict and a
  defaulted `OptionalModule::advise` hook. `KEPLER_FOCUS=off` disables it.
- **Sentinel** (`kepler-sentinel`, `kepler-git`, ADR-0015): deterministic path rules on `files.changed`, diff-line
  rules over `git diff`, `audit_findings` table (migration v3), `kepler sentinel audit|list|dismiss`.
- **Vision** (`kepler-vision`, `workers/vision`, ADR-0016): lazy worker manager with timeouts, capped output,
  minimal environment and a localhost allowlist; private TypeScript/Playwright worker; `kepler vision
  check|status|clean`. Worker verified only up to browser launch.
- **Orbit** (`kepler-orbit`, ADR-0017): DAG planner with cycle detection, bounded concurrency, git worktree per task,
  dependency merges and conflict detection, approval checkpoints for destructive commands; `orbit_runs`, `tasks` and
  `task_dependencies` tables (migration v4); `kepler orbit plan|run|list|status|approve|deny|resume|cancel|cleanup`.
- ADR-0018: Genesis stays a design (no UI implemented), with prerequisites.
- `cargo deny` configuration check, `cargo doc` warning gate and a worker CI workflow (`vision-worker-check.yml`).
- Root README rewritten with an honest status, installation, usage, architecture and command reference.

### Changed
- `rusqlite` 0.32 -> 0.41 (bundled SQLite 3.54) to fix RUSTSEC-2026-0459 found by `cargo deny check`; minimum Rust is now 1.89; `Zlib` allowed in `deny.toml` (ADR-0019). FTS5 retrieval measured ~1.5-2x slower in a micro-benchmark, still < 1 ms.
- `deny.toml` validated with cargo-deny 0.22.0 (advisories, bans, licenses, sources ok).
- All GitHub Actions in `ci.yml` are pinned to commit SHAs; checkouts no longer persist credentials.
- Test temp directories use counters instead of timestamps (timer-resolution collisions on Windows).

### Earlier deliveries on this branch (see git history and the backlog)
- Delivery 0: event envelope v1, capability negotiation, policy engine, IPC, SQLite storage, daemon, `kepler doctor|emit|daemon|version`.
- Delivery 1: Forge, Memory, `kepler memory ...`.
- Delivery 2: Guardian, `kepler mcp serve`, `kepler inspect`, Pi adapter hooks, performance measurements.

### Known gaps
No verification against a real Pi session, a third-party MCP client, a real Chromium, Linux or macOS; no
benchmark with/without Kepler; no Genesis; release pipeline and installers never run against a real release.
