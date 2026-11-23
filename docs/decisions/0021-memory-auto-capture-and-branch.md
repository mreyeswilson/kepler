# ADR-0021: Memory captures observed facts and selects branch scope

Status: accepted. Amends ADR-0010 and the v1 event contract (additive, optional fields).

## Context
Nothing created memories from a real session unless an adapter or the user sent an explicit
`memory.candidate`, and the `branch` scope could never be selected because v1 events carried no
branch.

## Decision
1. `input.received` and `agent.started` accept an optional `branch` (bounded short string).
   Old adapters keep working (field optional); a new adapter talking to an old core is rejected
   by `deny_unknown_fields`, which the adapter treats as passthrough. The core keeps the last
   branch per session (bounded) and uses it as `Ctx.branch` for retrieval and candidate scope.
2. Memory derives candidates from what it **observed**, never from model text:
   * a build/test/lint command (`cargo test|build|clippy|check|fmt`, `npm|pnpm|yarn run
     test|build|lint`, `pytest`, `go test`, `dotnet test`, `make test`, ...; single simple
     command, no chaining/pipes/substitution) whose `tool.call.completed` reported `ok` ->
     "Observed: `<cmd>` completed successfully" (category `verified_command`, project scope);
   * files reported by `files.changed` (sensitive paths dropped, max 16) when the session
     settles `ok` and a branch is known -> "the last completed task on branch X changed: ..."
     (category `branch_work`, **branch scope**).
3. Both are ordinary pending candidates: low trust (`derived`), persisted only on an `ok`
   settlement, redacted, deduped, never superseding user facts, visible with
   `kepler memory list`.
4. Tool-call events reach modules only through `advise` (the Guardian decides their verdict), so
   the capture lives there and produces no advisories.

## Limits
Tool calls are paired per session (last requested command with the next completed call of the
same tool); concurrent tool calls in one session could mispair, which at worst skips or misattributes
a "verified command". Ranking weights are unchanged and were not tuned.
