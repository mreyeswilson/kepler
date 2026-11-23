# Kepler Genesis — interface design intelligence

**Status:** planned post-MVP module; **not implemented**, no UI exists. The implementation path and prerequisites are in [ADR-0018](../decisions/0018-genesis-deferred-design.md).

## Purpose
Turn user requests into professional, consistent UI designs and implementation plans. Unlike **Vision**, which inspects existing rendered UI, **Genesis** proposes layouts, design systems and screen/component solutions.

## Capabilities
- Infer UI-design intent without taking over ordinary code changes.
- Analyze current framework, existing components, design tokens and route structure.
- Create design specifications: layout, typography, spacing, colors, accessibility and responsive behavior.
- Provide wireframe/mockup candidates and bounded design variations.
- Map approved designs to existing components (React, Vue, Angular, Svelte; other stacks via adapters).
- Generate component and code-change plans for the **existing coding agent** to implement.
- Validate generated results using Vision and Sentinel; present discrepancies as evidence and avoid infinite repair loops.
- Preserve functional logic during redesign unless requested otherwise.

## Lifecycle
```text
User design request
 -> Forge requirements + Memory project design context
 -> Focus checks scope, goals and budget
 -> Genesis prepares tokens, layout, design alternatives
 -> optional preview on demand
 -> user approval for major visual changes
 -> coding agent implements targeted changes
 -> Vision/Sentinel verify
 -> Focus checks completion and stops unnecessary iterations
```

## Architecture
- Rust module for intent, constraints, design specifications and task orchestration.
- Optional TypeScript renderer with Playwright/preview worker launched on demand.
- Model use through a supported, explicitly configured harness/provider; no bundled heavyweight model or cloud dependency.
- Typed, versioned design output schema (to be established during implementation).
- Component catalogs and project design systems preferred over invented UI libraries.

## Guardrails
- No browser/render worker for simple backend changes.
- Honor existing stack, design system and responsive requirements.
- Treat reference images as untrusted sources and respect IP/licensing.
- Do not claim pixel-perfect recreation without visual verification.
- Approval before replacing an established design language or making major layout changes.
- Limit design revisions by Focus budgets and acceptance criteria.

## Acceptance tests
- React, Angular, Vue and Svelte representative tasks.
- Responsive overflow and accessibility checks via Vision.
- Design-token reuse and low irrelevant file churn.
- Measure added runtime/memory/token costs against benefit.
