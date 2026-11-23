# Kepler Focus — central execution control

**Status:** specification only; not implemented.

## Mission
Keep coding agents aligned with the original user request, grounded in evidence, minimal in scope and efficient in time/tools. Focus is a deterministic-first policy system, not another LLM agent.

## Six mechanisms
1. **Scope Lock:** immutable original request, explicit confirmed acceptance criteria, flexible expected file scope and review of unrelated edits. Do not impose a rigid list when justified changes are needed.
2. **Evidence Gate:** statements are observed, inferred or unknown. File existence, API behavior, tests and completed work must be backed by appropriate evidence.
3. **Execution Budget:** adaptive thresholds on elapsed time, calls, retries, tokens (when measurable), CPU/RAM. On exceed, reassess or warn; never blindly kill productive work.
4. **Loop Detector:** fingerprint commands, outcomes and diffs; detect repeated failures, duplicate reads and oscillations while allowing changed hypotheses and legitimate retries.
5. **Minimal Change Guard:** review diff relevance and disproportionate refactors/dependencies; allow justified exceptions and preserve pre-existing user edits.
6. **Completion Gate:** compare observed evidence to criteria and recommend concluding when satisfied; do not continue unrequested polishing.

## Event and policy outcomes
Input/task state: original prompt, requirements, assumptions, task ID, repository, scope, progress evidence, retries, budgets, diff signatures and capabilities.

Policy outcomes: `allow`, `warn`, `request_approval`, `recommend_stop`, `block`. Actual blocking/cancellation exists only where harness permits interception; no universal promise.

## Efficient execution
Fast local rules first; short budgets for supervision hooks; bounded queues; no mandatory LLM. Distinguish waiting for remote actions from productive work and loops.

## Defaults
Suggested complexity-based time/tool budgets are **hypotheses for calibration**, never SLA or universal hard limits. Allow user policies, dynamic adjustment and explicit override.

## Example
User: "Add a logout button." Identify the relevant component and existing logout mechanism, implement minimal change, verify targeted behavior, report completion. Challenge unrelated authentication rewrites or migrations unless evidenced as necessary.

## Validation
Track correctness, false-positive drift/loop warnings, unnecessary diffs, unsupported claims, p95 overhead and completion time. See [benchmark protocol](../quality/BENCHMARKS.md).
