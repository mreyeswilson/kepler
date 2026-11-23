# Performance, accuracy and scope benchmark

**Status:** proposed protocol; no results measured.

## Goal
Validate that Kepler improves end-to-end developer outcomes, not only token counts or number of steps.

## Dataset
Create **30–50 versioned tasks** with fixtures and acceptance tests: small button, endpoint flag, SQL query correction, regression fix, form, responsiveness issue, medium feature and multi-file change. Use real representative repositories with confidential information removed.

## Conditions
Run each task with Kepler enabled and disabled using the same harness/model versions, initial repo state, tool permissions and resources. Repeat runs and report variance; randomize ordering to reduce bias. Note external network/model variability and use cold/warm runs.

## Metrics
- Correctness: passing acceptance tests and verified behavior.
- Scope: irrelevant modified files, unexpected dependencies, unrequested architecture changes.
- Evidence: ungrounded assertions about files, tests, APIs or completion.
- Efficiency: wall time, model/tool calls, retries and wasted loops.
- Cost: measured tokens when available versus estimated tokens explicitly labeled.
- Overhead: startup, RSS, CPU, p50/p95/p99 hook latency, peak worker usage.
- Safety: unjustified cancellations and Focus false positives.

## Release gate
Do not announce speedups or hallucination elimination without controlled evidence. A faster failure is not a success. Report regressions, uncertainty and applicability per harness.

## Focus-specific tests
Inject deliberately redundant tool loops, stale memory, missing files and tempting unrelated refactors. Verify Focus warns correctly without disrupting necessary edits or productive retries.

## Genesis-specific tests
Compare target design spec to rendered UI, reuse of existing components, accessibility, responsive behavior, and whether design generation increases overall task time disproportionately.
