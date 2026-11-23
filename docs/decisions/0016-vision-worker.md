# ADR-0016: Vision worker (Playwright over stdio JSON)

**Status:** Accepted. Rust manager and the TypeScript worker are implemented; the worker has **not** been run
against a real browser (no browser was downloaded).

## Context
Delivery 4: demand-started UI inspection without a resident Chromium, with time and memory limits, a URL allowlist
and no sensitive persistence. Playwright is a Node library; the core is Rust and must not link a browser stack.

## Decision
* `kepler-vision` (Rust, std only plus serde) is the manager: **lazy** (no process until a valid `check`; a URL
  that fails the allowlist, a bad viewport list or an out-of-range timeout is rejected before any spawn),
  one worker at a time, JSON lines over stdio, per-request wall-clock timeout (+5 s grace) with kill, response
  lines capped at 300 KiB, stray non-JSON output tolerated up to 200 lines, minimal environment
  (`env_clear` plus a pass-through list and `KEPLER_VISION_*`; tokens and cloud credentials are not forwarded),
  respawn on the next call after any failure, shutdown on drop.
* Allowlist: `http(s)` to `localhost`, `127.0.0.1`, `[::1]`, `*.localhost` by default; extra hosts only through
  `KEPLER_VISION_ALLOW_HOSTS`; no credentials in URLs. Implemented twice (Rust and TS) and applied by the worker to
  every sub-request and redirect via request interception.
* Worker (`workers/vision`, **private**, TypeScript on Node >= 22.18): protocol v1 (`hello`, `check`, `shutdown`),
  a fresh browser context per viewport, DOM-only probe (horizontal overflow, elements wider than the viewport,
  missing alt/labels/names/lang/title/viewport meta), console and page errors, failed requests with query strings
  stripped, opt-in screenshots written to a directory chosen by the manager. Playwright is imported dynamically
  only when a check runs; its absence or a missing Chromium maps to stable codes (`playwright_unavailable`,
  `browser_unavailable`). Browsers are never downloaded by Kepler.
* Limits: V8 heap flag, an RSS guard that refuses work and exits, 60 s idle exit, closing the browser on timeout.
  Chromium's own RSS is **not** capped (no job objects/cgroups; `unsafe` is denied workspace-wide).
* Findings: results map to `audit_findings` with `source = vision` (ADR-0015); only counts, rule ids, short
  selectors and redacted clipped messages are kept. Screenshots are files under `<data dir>/vision/`, never in
  the database; `kepler vision clean` deletes them.
* The worker is **not** started by the daemon and no event triggers it: the Pi adapter and daemon do not call
  Vision. `kepler vision check|status|clean` is the only entry point; it stays off until `KEPLER_VISION_WORKER`
  is set.

## Consequences
Zero idle cost and a small attack surface, but real-browser behavior, CSS-dependent checks, contrast and
accessibility-tree analysis are unverified or absent. The checks are smoke tests, not an audit.

## Alternatives
A resident browser service (idle RAM), embedding a browser crate in Rust (heavy dependency, needs an ADR), an HTTP
worker protocol (port exposure).
