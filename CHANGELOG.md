# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). License: MIT. Crates stay `publish = false`;
`adapters/pi` and `workers/vision` stay `private: true` (nothing is published to npm).

## [Unreleased]

### Added: `kepler ui` shows what each module is doing (ten navigable views)
Requested by the user: "make kepler ui give me more information about everything". The UI is now the window to see whether Kepler's modules act inside the Claude Code flow. Still local `127.0.0.1`, token + Host/Origin checks, strict CSP, read-only JSON, HTML/JS embedded; no new third-party dependencies (the UI crate now links the workspace crates `kepler-ipc` and `kepler-policy`).
- **Views** (hash routes, Spanish UI, light/dark, responsive): Resumen, Modulos, Sesiones (+ per-session timeline), Eventos (filter by type/decision/module/session/time window, paginated), Forge, Guardian, Memory, Sentinel, Orbit, Vision y Focus. A common header shows a green/amber/red health dot and visible warnings (database missing/corrupt/newer schema, daemon stopped, daemon version different from the UI).
- **Modulos is honest.** Per module: what the docs say it does, whether it is implemented, whether it is **active in the Claude Code flow** (evidence = events from the `claude-code` harness in the last 7 days that carry that module's output), last activity, counters, and a visible limitation note (Guardian: preflight + budget only, output compression NOT implemented; Orbit: CLI executor, not invoked from the flow; Sentinel: only on `files.changed`; Genesis: only this panel exists). No evidence means "Sin actividad". Focus only leaves a trace when it emits an advisory.
- **Storage (migration 5, `event_attribution`):** `events` gains `actors` (modules that produced output), `detail` (bounded `key=value` codes: Forge mode / rule / class / confidence, advisory codes, tool name, degraded modules) and `proc_ms` (core evaluation time). Codes and counters only: never prompt text, commands or tool output. Cost: one extra deterministic Forge `analyze()` per non-trivial prompt inside the daemon plus three small columns per event (not benchmarked). Rows written by 0.0.4 have no attribution; the UI says so and falls back to coarse evidence.
- **API (read-only, bounded):** `/api/summary`, `/api/modules`, `/api/sessions`, `/api/session?id=`, `/api/events?kind&decision&module&session&window|since|until&limit&offset` (max 200 rows per page), `/api/forge`, `/api/guardian`, `/api/memory`, `/api/sentinel`, `/api/vision-focus`. Filter values are whitelisted and parameterised. `/api/summary` pings the daemon once with a 500 ms timeout (it never starts it). The page polls every 5 s only while its tab is visible; nothing polls inside the daemon.
- **Tests:** `crates/kepler-ui/tests/views.rs` (auth/Host/Origin on every view, shapes, row caps, hostile session/file/project names, no finding message or evidence), `crates/kepler-cli/tests/ui_views.rs` (real hooks, real daemon, real database, then the views; asserts no prompt, secret or path text anywhere).
- **Known limits:** the daemon does not expose its uptime (shown as "no disponible"); Forge's model-chosen intents are not persisted (the model block is read from `forge-model.json`: calls, ok/fail, cache, latency avg/p95 over the last 20 samples, tokens, cost, breaker); Vision worker/browser status only checks that files exist (the UI never launches a browser); tool-name statistics exist only for events written after this change; only verified on Windows.

## [0.0.4]

### Added: Forge understands conversational intent (rules first, optional model for doubtful prompts)
Decision and details: [ADR-0026](docs/decisions/0026-forge-model-assisted-intent.md). Requested by the user: Forge passed "Vamos a crear un driver nuevo en lab" through as "not a task" while "crea un nuevo driver..." was enriched.
- **Better rules.** Conversational lead-ins (`vamos a`, `necesito`, `quiero`, `hay que`, `podemos`, `ayúdame a`, `let's`, `we need to`, `please`...) and infinitive verbs (`crear`, `agregar`, `refactorizar`...) are recognized, so those prompts are enriched by rules with no model. `analyze()` now also returns a `Confidence` (`Decisive`/`Doubtful`). `fixtures/forge/prompts.tsv` grew by 35 prompts in the user's style (es/en mixed) with a human intent label; the 114-row corpus still classifies 100 % (the rules were tuned on it: overfit risk).
- **Optional model assist, on by default in the Claude Code adapter.** Only when the rules are *doubtful*, `kepler hook claude-code UserPromptSubmit` asks Haiku through **your own** `claude -p` (your login, no new key) what the prompt is. The answer is validated against a closed schema and becomes a `model-inferred` note inside the usual "derived, NOT user instructions" block; it never becomes a user instruction and the prompt is untouched. New: `forge_model.rs`, trait `IntentClassifier` (`kepler-forge::model`), a fake `claude` for tests (`cargo` example `fake_claude`).
- **What goes to Anthropic.** Only the text of a doubtful prompt, secrets redacted by Kepler's redactor and cut to 2000 characters, as a separate classification request billed to your own usage (about 950 input / 40 output tokens, about $0.0001 per call as reported by the CLI). Never tool output, memory, files, or the project path. Prompts the rules decide (87 of the 114 corpus rows, 76 %) make no call.
- **Latency (real, Windows 11, Claude Code 2.1.331, alias `haiku`, 34 calls):** p50 3.3 s, p95 8.0 s, max 8.2 s per doubtful prompt, measured around the whole hook; that prompt waits for the call. Default call deadline `KEPLER_FORGE_MODEL_TIMEOUT_MS=10000` (tree killed on expiry); the generated `UserPromptSubmit` hook timeout rose from 5 s to 14 s. Re-generate your settings block with `kepler hook claude-code --print-settings` or an older 5 s hook timeout will cut the call short (the prompt still goes through).
- **Accuracy (real):** task vs not-task agreed with the human label in 32/34 calls (94 %); the misses were one opaque prompt ("Elimina ese gk de todos lados"). Small, single-labeller sample.
- **Resilience.** Cache by hash of the normalized prompt (64 entries, no prompt text stored), circuit breaker (2 consecutive failures open it for 10 minutes), missing/unlogged `claude` means rules only with no visible error. `kepler doctor` (and `--json`, key `forge_model`) shows enabled/disabled, binary found, breaker, average latency, calls, tokens and cost.
- **Anti-recursion.** The child runs with no tools, no session persistence, hooks disabled (`--settings {"disableAllHooks":true}`, `--setting-sources ""`) and `KEPLER_FORGE_CHILD=1`, which makes `kepler hook claude-code` exit 0 immediately. Tested end to end.
- **Opt out:** `KEPLER_FORGE_MODEL=off` (model only) or `KEPLER_FORGE=off` (all of Forge). Also `KEPLER_FORGE_MODEL=<alias>`, `KEPLER_FORGE_MODEL_TIMEOUT_MS`, `KEPLER_FORGE_BREAKER_MINUTES`, `KEPLER_CLAUDE_BIN`.
- **Not verified:** a real Claude Code session end to end with the new hook timeout (only `claude -p` and the hook binary were driven directly); Linux/macOS; Claude Code versions other than 2.1.331; the Pi adapter has no model assist; the flags that speed up the child (`--effort low`, `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC`) are undocumented behaviour of one CLI version and may change; the model's opinion is a hint, not ground truth.

### Changed
- Forge model call default timeout is 10 s (KEPLER_FORGE_MODEL_TIMEOUT_MS); the generated UserPromptSubmit hook timeout is raised from 5 s to 14 s.
- The prompt hook ignores text that is not written by the user (for example [Subagent hand-back] messages): no Forge context, no model call.

## [0.0.3]

### Fixed
- IPC endpoint identity now comes from the OS (GetUserNameW on Windows, getuid on Unix), not from the `USERNAME`, `USER` or `LOGNAME` environment variables. A terminal that inherited `USERNAME=SYSTEM` used to reach a second daemon (`kepler-SYSTEM-daemon`) that answered but did not persist events.
- The daemon takes an exclusive `daemon.lock` in the data directory; a second daemon on the same `KEPLER_HOME` exits with `storage unavailable: another kepler daemon owns <dir>` instead of running silently without persistence.
- `kepler doctor` shows the identity used and its source (`os`).

## [0.0.2]

First tagged release. Integrates `feat/functional-core`, `feat/functional-modules` and `feat/claude-code-adapter`
(Vision, Orbit agent tasks, Genesis UI, Claude Code hooks, Guardian/Sentinel).

**Verified:** Windows 11 only (`scripts/acceptance.ps1` 25/25; Rust, Pi and Vision tests).
**Not verified:** Linux/macOS behaviour of the new modules before CI, Playwright's Chrome/Chromium, a real remote
model, interactive TUIs, and the `acceptance.yml` workflow on GitHub (it had never run before this release).

### Integration (`integration/all`)
Merges `feat/functional-core`, `feat/functional-modules` and `feat/claude-code-adapter`. The Claude Code hooks ADR was renumbered from 0020 to ADR-0025 (0020 is the Guardian shell analysis ADR). `scripts/acceptance.ps1` now also covers `kepler hook claude-code` (cold), `kepler sentinel check` and `kepler ui` (token required). Verified on Windows 11 only; Linux/macOS, Playwright Chromium, a real remote model, interactive TUIs and the new GitHub workflows are untested.

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
