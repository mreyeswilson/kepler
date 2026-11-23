# Vision

## Problem
Coding agents fragment knowledge across sessions and vendors, repeatedly consume context, produce uneven instructions, and require manual quality checks.

## Product
Kepler is a small, local-first runtime that enhances **existing** agents. It does not replace their chat interface, tools or permission model.

### Users
Developers operating multiple coding harnesses on Linux, macOS, or Windows.

### Desired experience
Install Kepler once. Write normal instructions. Compatible hooks trigger prompt enrichment, relevant memory retrieval, adaptive token filtering and targeted reviews. Kepler stays quiet when no action adds value.

### Six modules
Forge (prompt compiler), Memory (knowledge retention), Guardian (context), Orbit (orchestration), Sentinel (code audit), Vision (UI inspection).

### Design principles
- Minimal idle resource use; no browser, LLM or heavy indexer in the base process.
- Real capability negotiation rather than blanket “works everywhere” claims.
- Deterministic local rules first; model use is optional and explicit.
- Default private data stays local. Sensitive content must be filtered and controllable.
- Traceable, reversible, transparent changes.
- Open-source intent; license selection is a separate decision.

### Non-goals for initial MVP
New IDE, hosted cloud sync, full multi-agent scheduler, bundled LLM, mandatory telemetry, persistent Chromium and universal interception of proprietary agent prompts.

### Success measures
Measured changes to task completion, context quality, accuracy, p95 latency, RAM, CPU and confirmed token use where exposed.
