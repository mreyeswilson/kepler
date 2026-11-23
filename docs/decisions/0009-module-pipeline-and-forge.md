# ADR-0009: Module pipeline, derived-context labels and Forge

**Status:** Accepted (implemented in Delivery 1: `kepler-policy`, `kepler-forge`)

## Context
Delivery 0 had a policy engine with an `OptionalModule` hook but no module. Delivery 1 adds Forge (input
classifier/derived spec), Memory and, in Delivery 2, Guardian. `docs/architecture/EVENTS_AND_CAPABILITIES.md`
fixes the order: original -> Forge -> Memory -> Guardian -> harness. The hard rules from AGENTS.md apply:
the user's original prompt is preserved, inferences are never promoted to confirmed instructions, and a
failing optional module must leave the original intact.

## Decision

### Engine contract (`kepler-policy`)
* `OptionalModule` returns `Vec<Enrichment>` (not one string) and has three entry points:
  `on_input` (`input.received`, non-trivial only), `on_context_request` (`context.requested`) and
  `on_observe` (every other non-tool event: settlement, memory candidates, session close...).
* Modules run **in registration order**, which is the pipeline order. `PolicyEngine::pipeline()` reports it.
* `OutputStage` (Guardian) runs after all modules over the collected derived items; it may drop or dedupe,
  never add. A failing stage leaves the list as the modules produced it.
* Every call is wrapped in `catch_unwind`, charged against one time budget per event (default 50 ms), and its
  output is size-checked (8 KiB per item, 16 KiB total). Error, panic, slow, oversize or NUL => that module's
  output is dropped, a short note goes to `degraded[]`, and the verdict is whatever the other modules produced
  (usually `passthrough`). The original is never touched because the engine only returns a verdict next to it.
* Tool preflight is **not** an optional module: it runs on `tool.call.requested`, independent of modules, and fails closed.

### Labels on derived text
`Derived.provenance` is `inferred` (rules over the user's text, Forge) or `recalled` (read back from local
memory, Memory). Neither is ever a user instruction; adapters must present them as context, not as the prompt.
`recalled` is an additive wire value (ADR-0007 listed only `inferred`); adapters that do not know it must treat any
non-`user` provenance as non-authoritative. Protocol version stays 1.

### Forge (`kepler-forge`)
Deterministic, no model/network/I-O. `analyze(text)` returns `Analysis { mode, reason, spec }` and keeps the original
byte for byte (`Analysis::original()`).

| Mode | When (rules, in order) | Output |
|---|---|---|
| passthrough | trivial input; question-like text; no task verb; or a task that already names a file/identifier ("specific") | nothing |
| decompose | >= 3 list items, or >= 3 sentences with >= 2 sequence words (then, finally, luego...) | ordered steps (verbatim quotes) + constraints/acceptance quotes |
| compile | >= 160 chars with explicit constraint/acceptance sentences | objective (first sentence), constraints, acceptance, open questions |
| enrich | short task with no file/identifier target | open questions ("no target named", "no acceptance criterion") and an inferred task type |

Derived spec claims carry a `Label`: `user-stated` (only constructible via `Claim::stated(original, quote)`, which
rejects anything that is not a verbatim substring), `extracted` (file names/identifiers pulled mechanically),
`inferred` and `unknown`. The type has no path from an inferred claim to a stated one. The rendered block (<= 2 KiB)
always starts with a disclaimer that it is not a user instruction. Rules are heuristic, English/Spanish keyword based
and **not evaluated for quality on real prompts**; false positives cost a few tokens, false negatives cost nothing
(passthrough). `KEPLER_FORGE=off` disables the module.

## Consequences
Cost is one pass over <= 16 KiB of text per substantive input, no allocation beyond the result. Forge cannot
change intent because it cannot write to the input. A rewritten prompt ("compile" in the strong sense) is
deliberately *not* implemented: compile/decompose only add a separate labeled block.

## Alternatives
Letting Forge return a rewritten prompt (violates "never silently change requested intent"); an LLM classifier
(violates "deterministic classification before any model call", adds latency/cost/privacy exposure);
a regex engine (new dependency, ReDoS surface) instead of hand-written scanners.
