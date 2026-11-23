# Contributing to Kepler

Kepler is a Rust-first, local-first middleware for coding agents. The code base is young and unreleased; read
[README.md](README.md) (honest status), [AGENTS.md](AGENTS.md), the [MVP roadmap](docs/roadmap/MVP.md), the
[backlog and status table](docs/roadmap/BACKLOG.md) and the relevant [ADRs](docs/decisions/README.md) before changing anything.

## Workflow
1. Open or pick a scoped issue with acceptance criteria (backlog IDs such as `KPL-015` are planning identifiers).
2. Architecture or dependency changes need a **new ADR first** (next free number; format: Context, Decision,
   Consequences, Alternatives, Status). No new runtime or cloud dependency without one.
3. Small branches, one logical change per commit, focused pull requests following the PR template.
4. Tests and docs ship with the code. **Do not claim a function works until code and tests exist**, and say plainly what
   could not be verified (real Pi, real browser, other OSes).

## Local checks (all must pass before a PR)
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # Orbit and Sentinel tests run real `git` in temporary repositories
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo build --release             # optional locally, run in release workflows
```
Node packages (Node >= 22.18; both are `private: true`, never publish them):
```bash
cd adapters/pi     && npm ci --ignore-scripts && npm run typecheck && npm run build && npm test
cd workers/vision  && npm ci --ignore-scripts && npm run typecheck && npm run build && npm test
```
`cargo deny check` (config in `deny.toml`) runs in `security.yml`; run it locally if you have `cargo-deny` installed.
On Windows without Node on `PATH`, add your Node directory first (for example the nvm directory).

## Engineering standards
- Stable Rust, Tokio for I/O, SQLite for persistence; thin adapters; harness-specific calls stay outside the core.
- Everything bounded: queues, concurrency, payloads, output, time. Workers and browsers load lazily, never at boot.
- Events carry version, provenance and correlation/session IDs; payloads are bounded.
- Deterministic classification before any model call; an optional optimization must never block ordinary agent work,
  security boundaries fail closed.
- `unwrap`/`expect`/`panic`/`todo` are lint warnings (errors in CI) outside tests.
- Interventions must be disableable. Benchmark or estimate CPU/RAM/latency for hot paths and note it in the PR.
- Test temp directories use a counter, not a timestamp (Windows timer resolution causes collisions).

## Security and privacy
- Never persist API keys, `.env` content, prompts, command output or unredacted sensitive data. Redact before storing.
- Use redacted fixtures in tests, issues and logs. Report suspected vulnerabilities privately to the maintainers.
- Destructive actions need explicit approval; do not add code that bypasses the Guardian preflight.
- `adapters/pi` and `workers/vision` stay `private: true`; release gates are in [RELEASING.md](RELEASING.md).

## License
Licensing has not been selected; do not assume rights to reuse code outside GitHub's applicable terms until a
license is published.
