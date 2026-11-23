# ADR-0023: Focus calibrated on labelled traces; Pi shows advisories and reports edits

Status: accepted. Amends ADR-0014 and ADR-0013.

## Decision
1. **Calibration harness** (`crates/kepler-focus/tests/calibration.rs`): hand-written session
   traces (loops, ping-pong edits, scope drift, unrequested dependency, polishing after a
   passing test, unverified completion, and normal work that must stay quiet) with the advisories
   a reviewer would want and the ones that would be noise. The test prints detection, false
   positives and per-event cost and fails on regression. Current result with default thresholds:
   9/9 expected advisories, 0 false positives on the 5 quiet traces, about 84 us/event in a release
   build (includes the Guardian preflight of tool calls).
2. **One threshold change found by the harness**: `loop.failure_streak` now resets when files
   change between failures. Six failing test runs with an edit in between is iteration, not a loop
   (a retry after a change is legitimate, consistent with the rest of Focus).
3. **Pi adapter**: advisories on any verdict are shown with `ctx.ui.notify` when Pi has a UI
   (`hasUI`), once per code per session, sanitized and bounded; nothing is shown in print mode.
   The adapter also reports `files.changed` after a successful `edit`/`write` tool, which is what
   feeds Focus scope/loop detection, Sentinel path rules and branch memory (ADR-0021), and sends
   the git branch (read from `.git/HEAD`, no process spawned).

## Honest limits
The traces are synthetic and written by the same author as the thresholds; they demonstrate the
detectors work as designed and that obvious normal sessions stay quiet, **not** that thresholds
are optimal for real agents. The with/without-Kepler comparison on 30-50 real coding tasks
(KPL-032) needs a live model and was not run. Edits made through `bash` are invisible to
`files.changed`. Advisory display was verified by unit tests with a fake `ui.notify`, not in a
real Pi TUI session.
