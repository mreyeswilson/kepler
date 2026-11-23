# @kepler/vision-worker

Private package (`"private": true`, never published). On-demand UI inspection for `kepler vision`:
a Node process that opens a page with Playwright's Chromium and reports facts, over JSON lines on
stdio (protocol v1, `src/protocol.ts`, mirrored by the Rust crate `kepler-vision`).

## What is verified and what is not
* Verified by tests: URL allowlist, protocol validation, timeouts, memory guard, idle expiry,
  sanitization, a real process answering `hello`, and the Rust manager end to end up to the point of
  launching a browser (`browser_unavailable` when Chromium is missing).
* **Not verified:** any run against a real Chromium page. No browser was downloaded when this was
  written, so the DOM probe (`src/probe.ts`), screenshots and request interception are untested
  against a real browser.

## Use
```
npm ci --ignore-scripts && npm run build
npx playwright install chromium        # explicit, user-run; Kepler never downloads a browser
KEPLER_VISION_WORKER=<repo>/workers/vision/dist/src/main.js kepler vision check http://localhost:3000
```
Nothing starts at boot: the worker exists only while `kepler vision check` runs, and closes its
browser after 60 s idle, on shutdown, on timeout and when stdin closes.

## Limits and privacy
* Only `http(s)` to `localhost`, `127.0.0.1`, `[::1]`, `*.localhost` unless `KEPLER_VISION_ALLOW_HOSTS`
  lists more; every sub-request and redirect is checked too; downloads, service workers and
  credentials in URLs are refused.
* Memory: `--max-old-space-size=384` for the V8 heap plus an RSS guard (`KEPLER_VISION_MAX_RSS_MB`,
  default 768) that refuses work and exits. Chromium's own memory is not limited by Kepler.
* Time: per-request timeout (default 15 s, max 120 s); the manager kills a worker that overruns.
* Output: results are bounded (4 viewports, 10 messages of 200 chars each), URL query strings and
  fragments are removed, screenshots are opt-in files in the Kepler data directory and never
  embedded in results or the database.
* The worker runs with a minimal environment (no tokens or cloud credentials are forwarded).
