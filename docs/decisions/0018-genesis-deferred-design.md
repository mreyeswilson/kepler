# ADR-0018: Genesis stays a design only; what exists to build it on

**Status:** Accepted. Genesis is **not implemented** and no desktop UI was created.

## Context
Genesis (ADR-0006) is the post-MVP interface-design module. The request for this delivery was to build it only
if reasonable and not to invent a UI nobody asked for. Genesis needs three things this repository did not have a
year of design for: a typed design-specification schema, a project design-token extractor, and a model-backed
design step that runs through a harness the user configured. A desktop window additionally needs a UI toolkit
(Tauri, Electron or similar), a heavy runtime dependency that AGENTS.md forbids adding without its own ADR and
that conflicts with the low idle-cost goal.

## Decision
* Do **not** implement Genesis or any desktop UI now. Nothing in the code, CLI or docs claims otherwise.
* Genesis will be a *composition* of pieces that now exist, in this order:
  1. **Design context** (Rust, deterministic): read tokens from the project (CSS variables, Tailwind/theme files,
     `design-tokens` JSON) into a bounded, versioned `design.context` record with provenance. Pure file reading
     inside a user-approved root, reusing the path guard and redaction of `kepler-policy`.
  2. **Design specification schema** (versioned JSON, new ADR before coding): layout regions, spacing/typography
     scales, component mapping, responsive breakpoints, accessibility requirements, variants (max 3), each field
     labeled `user`, `inferred` or `extracted` exactly like the Forge spec. Inferences are never promoted.
  3. **Approval checkpoint**: material visual changes reuse the Orbit approval mechanism (`awaiting_approval`,
     one-shot grant) and Focus scope rules; a design replaced without approval is a defect.
  4. **Implementation** stays with the user's coding agent (Genesis only produces the plan); Orbit may run the
     per-component tasks in worktrees.
  5. **Verification**: `kepler vision check` (ADR-0016) at the spec's breakpoints and `kepler sentinel audit`
     (ADR-0015); discrepancies are findings with evidence, and Focus budgets bound correction loops (<= 2 rounds).
* A preview, if ever wanted, is a **web page served locally on demand** and opened in the user's browser, not a
  bundled desktop shell; it needs no new runtime dependency and can be checked by Vision. A native shell would
  need an ADR with a measured idle-cost budget.
* Model calls (design variants) go only through a harness/provider the user configured; no bundled model, no
  cloud account, no automatic upload of source or screenshots.

## Acceptance criteria before any code
Schema ADR accepted; a fixture project per stack (React, Vue, Svelte, Angular) with expected extracted tokens; a
measured token-extraction budget (< 200 ms, < 2 MB on a mid-size repo); Vision verified against a real browser
(currently unverified, see ADR-0016); the benchmark in `docs/quality/BENCHMARKS.md` extended to design tasks.

## Consequences
The backlog items KPL-034..036 stay open and honest. The reusable pieces (findings table, approval flow, worker
protocol, allowlist) already exist, so Genesis later adds one module and one schema, not a new subsystem.

## Alternatives
A bundled desktop app now (heavy dependency, unrequested, unverifiable without a browser stack); a model-only
"design agent" with no structure (cannot be verified or bounded).
