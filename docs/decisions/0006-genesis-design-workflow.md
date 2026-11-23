# ADR-0006: Genesis as an on-demand design module

**Status:** Accepted for design; implementation deferred

## Context
Developers want to generate professional UI designs and matching code, but heavyweight rendering and iterative model calls can undermine Kepler's efficiency goals.

## Decision
Add Genesis as a seventh supporting module after core control features. Reuse project design tokens; generate structured specifications and previews on demand; the existing coding agent implements code and Vision/Sentinel verify. Focus controls scope and iteration budgets.

## Consequences
Consistent UI creation while preserving a lean base process; additional preview/model dependencies remain optional and lazy-loaded.

## Alternatives
Always-running design service (high overhead), code-only output with no preview (harder validation), replacing the coding agent (against product mission).
