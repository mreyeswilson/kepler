# Vision

## Problem
Coding agents may lose track of the requested goal, assert unverified facts, overengineer small changes, repeat failed attempts and take far too long on simple tasks. Fragmented memory and token waste amplify the problem.

## Product
Kepler is a local-first **execution-control runtime** that keeps existing coding agents focused, evidence-grounded, proportional in scope and efficient. It does not replace their chat interface, tools or permission model.

### Users
Developers operating multiple coding harnesses on Linux, macOS, or Windows.

### Desired experience
Install Kepler once. Write normal instructions. Compatible hooks trigger prompt enrichment, relevant memory retrieval, adaptive token filtering and targeted reviews. Kepler stays quiet when no action adds value.

### Control engine and seven modules
**Focus** is the central execution-control engine. Seven supporting modules: Forge (prompt compiler), Memory (knowledge), Guardian (context), Orbit (orchestration), Sentinel (code audit), Vision (UI inspection), and **Genesis** (UI design and code generation).

Genesis is a later-phase module, activated only for actual design tasks and validated by Vision/Sentinel.

### Design principles
- Minimal idle resource use; no browser, LLM or heavy indexer in the base process.
- Real capability negotiation rather than blanket “works everywhere” claims.
- Deterministic local rules first; model use is optional and explicit.
- Default private data stays local. Sensitive content must be filtered and controllable.
- Traceable, reversible, transparent changes.
- Open-source intent; license selection is a separate decision.

### Outcome measures
- Fewer unsupported claims, repeated calls and unrelated diffs.
- Higher verified acceptance-criteria completion.
- Reduced end-to-end time and resource use on equivalent tasks, without quality regression.
- Compare with/without Kepler under controlled conditions; report overhead and false positives.

### Non-goals for initial MVP
New IDE, hosted cloud sync, full multi-agent scheduler, bundled LLM, mandatory telemetry, persistent Chromium, immediate Genesis implementation, and universal interception of proprietary agent prompts.

### Success measures
Measured changes to task completion, context quality, accuracy, p95 latency, RAM, CPU and confirmed token use where exposed.
