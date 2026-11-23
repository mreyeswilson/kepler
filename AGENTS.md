# Instructions for coding agents

You are contributing to **Kepler**, a Rust-first, local-first AI agent middleware project.

## Before changes
- Read `README.md`, `docs/architecture/OVERVIEW.md`, `docs/roadmap/MVP.md`, and the relevant ADRs.
- Identify the feature scope and the harness capabilities available. Do not assume MCP alone enables interception.
- Preserve the user's original prompt; never promote inferred requirements to confirmed user instructions.

## Architecture rules
- Stable Rust for core, Tokio for async I/O, SQLite for persistence.
- Thin adapters; harness-specific API calls must stay outside the core.
- Bound task queues and concurrency. Lazy-load workers, browsers, embeddings, and LSPs.
- Avoid unnecessary allocations, dependencies, global background polling, or network calls.
- Every event has version, provenance, correlation/session IDs, and a bounded payload.
- No new runtime/cloud dependencies without an explicit ADR.

## Safety and quality
- Never persist API keys, .env content or unredacted sensitive tool output.
- Never execute destructive actions without approval.
- Prefer deterministic classification before any model call.
- Write unit/integration tests with each feature and document performance impacts.
- A failure in an optional optimization must not block ordinary agent work; security boundaries fail closed.
- Do not claim functions are implemented until code and tests exist.

## Pull request checklist
- Scope and design rationale
- Compatibility and capability fallback
- Security/privacy implications
- CPU/RAM/latency measurement or anticipated costs
- Tests and documentation updates
