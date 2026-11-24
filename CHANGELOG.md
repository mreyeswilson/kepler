# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/). License: MIT. Crates stay `publish = false`;
`adapters/pi` and `workers/vision` stay `private: true` (nothing is published to npm).

## [Unreleased]

### Added: Orbit in the Claude flow (MCP tools, skill, ADR-0030)
Requested by the user: Orbit could only be used from `kepler orbit` in a terminal; Claude had no way to delegate work to sub-agents. Decision and limits: [ADR-0030](docs/decisions/0030-mcp-tools-for-orbit-and-modules.md).
- **24 MCP tools** (was 3), same in-tree server and no new dependency: Orbit (`kepler_orbit_suggest_plan`, `_plan`, `_run`, `_list`, `_status`, `_logs`, `_approve`, `_deny`, `_resume`, `_cancel`, `_cleanup`), `kepler_forge_analyze`, `kepler_guardian_check`, `kepler_sentinel_audit/_list/_check`, `kepler_memory_list/_export/_add`, `kepler_vision_status/_check`. Each has a description that says when to use it, a strict JSON Schema (`additionalProperties: false`, validated by the server with the same schema) and MCP annotations (read-only / destructive / idempotent / open-world). Registry is one file per family so modules can add tools without touching the others.
- **`kepler_orbit_run` does not block**: it validates, creates the run, starts a detached executor and returns the run id in about a second; the model polls `kepler_orbit_status`. `agent` tasks (real `claude -p` in a worktree) are the delegation mechanism; `kepler_orbit_suggest_plan` builds the plan (two or more agents plus an optional integration task) deterministically, no model.
- **Guards:** `confirm: true` for approve and cleanup (descriptions say: only after the user's explicit yes in the chat); the Guardian is re-checked before an approval; permission-bypass flags in `agent_args` are refused; `KEPLER_MCP_READONLY=1` hides and refuses every tool with effects; outputs are redacted and cut at 32 KiB; `kepler_memory_add` stores agent-derived (not user-trusted) memories.
- **Claude Code integration files** in `integrations/claude-code/`: skill and `/kepler-orbit` command (frontmatter per the Claude Code docs), manual install steps, `claude mcp add` and Claude Desktop JSON for Windows, sample divide-and-delegate plan. Nothing is installed into `~/.claude`.
- **Changed:** out-of-range numeric arguments to `kepler_inspect` / `kepler_memory_search` are now rejected (-32602) instead of clamped; tool output limit 16 -> 32 KiB.
- **Tests:** unit tests for the schema validator, every tool's registration/annotations and the plan builder; stdio e2e tests with the real binary and a fake sub-agent (`fake_agent` example); `scripts/acceptance.ps1` gained 4 MCP steps (incl. the official MCP SDK for Node and, with `-WithClaude`, `claude -p` calling a tool).
- **Verified for real (Windows 11, Claude Code 2.1.331):** `@modelcontextprotocol/sdk` 1.20.0 listing and calling tools; `claude -p --mcp-config` calling `kepler_forge_analyze` and `kepler_orbit_plan`; a real run through MCP with two `claude -p` agent tasks creating files plus an integration task (branches and commits checked). **Not verified:** the Claude Desktop GUI, `claude mcp add` scope local (the test used `--mcp-config`, to leave `~/.claude.json` alone), a user session where Claude decides by itself to delegate, Linux/macOS.
- **Honest limits:** MCP still intercepts nothing; delegation happens only when the model calls the tools and the user agrees. Results stay on `kepler/orbit/<run>/<task>` branches; nothing merges for you. The detached executor may end with the client if its job object forbids breakaway (`kepler_orbit_resume` continues).

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
