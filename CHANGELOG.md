# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). Nothing has been released: no tags, binaries or npm
packages exist. License: MIT. Everything below is on branch `feat/rust-core-d0` and is
**unreleased** (workspace version `0.0.1`, `publish = false`).

## [Unreleased]

### Added (functional modules, Windows)
- **Vision runs against a real browser**: the worker launches an already-installed Microsoft Edge (`channel: msedge`), then Chrome, then Playwright's Chromium, or `KEPLER_VISION_BROWSER`; `kepler vision setup` installs the embedded worker sources plus the pinned `playwright` package into the data directory (no browser download, no postinstall); `kepler vision status --probe` launches the browser once and reports which one, or `browser: NONE` with the fix. New findings: HTTP 4xx/5xx sub-resources and broken same-origin links (`links.broken`); screenshots capped at 2 MiB. Worker process tree (browser included) is killed with `taskkill /T` on timeout.
- **Orbit `agent` tasks**: `{"agent": "<prompt>", "agent_args": [...]}` runs `claude -p` (or `KEPLER_AGENT_BIN`) as argv in the task worktree; task output is stored in separate stdout/stderr logs and redacted when the task ends; `kepler orbit logs <run> <task>`.
- **`scripts/acceptance.ps1`** and the `acceptance.yml` workflow (windows-2022): functional suite over the release binary with a PASS/FAIL/SKIP table; skips are never passes.

### Fixed (functional modules)
- Orbit `cleanup` only removed worktrees/branches whose paths were recorded in the database, so cancelled or killed tasks leaked a prunable worktree registration and their branch. It now derives both from the run and task ids.
- Orbit logs mislabelled output: both pipes shared one file and two headers were written up front, so stdout could appear under `--- stderr ---`.
- The Vision worker environment lacked `ProgramFiles*`, which Playwright needs to find an installed Edge/Chrome.

### Added
- **Claude Code adapter** (`kepler hook claude-code <Event>`, `--print-settings`, ADR-0025): Rust hooks for UserPromptSubmit, PreToolUse, PostToolUse, Stop, SessionStart and SessionEnd mapped to v1 events; Guardian deny/ask on PreToolUse, labeled derived context on prompts, secret warning on tool results; always exit 0, passthrough on any Kepler failure. Unit and e2e tests; run manually against Claude Code 2.1.330 on Windows only.
- **Private-repo testing** (anonymous download first; token only if `KEPLER_GITHUB_TOKEN` is set or as a fallback after a 401/403/404): `install.sh`/`install.ps1` accept an optional GitHub token (`KEPLER_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token`) and then download assets through the GitHub API (`Authorization` only to `https://api.github.com`, never printed or stored; SHA-256 still mandatory). Tests in `tests/installers/`; README section "Probar con el repo privado".

### Fixed
- **Release workflow**: `actions/attest-build-provenance` now runs only when the repository is not private (unsupported for user-owned private repos, which failed every build); a `::notice::` explains the skip. Upload and publish do not depend on attestation, and `gh attestation verify` only applies to releases built from a non-private repo.

### Added (earlier)
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
