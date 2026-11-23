import { mkdirSync, rmSync, statSync } from "node:fs";
import { join } from "node:path";
import { stripQuery, subRequestAllowed } from "./allowlist.ts";
import { domProbe } from "./probe.ts";
import type { Viewport, ViewportResult } from "./protocol.ts";

/** What the worker needs from a browser. The real one is Playwright; tests inject a fake. */
export interface BrowserDriver {
  /** Is the browser automation package importable? (Does not launch anything.) */
  available(): Promise<boolean>;
  /** Launch (or reuse) a browser and report which one was found, e.g. `msedge`. Throws `browser_unavailable`. */
  ensureBrowser(): Promise<string>;
  check(url: string, vp: Viewport, opts: { screenshotDir?: string }): Promise<ViewportResult & { title: string }>;
  /** Close the browser if one was launched. Safe to call repeatedly. */
  close(): Promise<void>;
}

export class DriverError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

/** Playwright reports a missing browser binary with a message containing one of these. */
export function mapLaunchError(e: unknown): DriverError {
  const msg = e instanceof Error ? e.message : String(e);
  if (/Executable doesn't exist|playwright install|browserType\.launch|distribution .* is not found/i.test(msg)) {
    return new DriverError(
      "browser_unavailable",
      "No usable browser found (tried KEPLER_VISION_BROWSER, Microsoft Edge, Google Chrome, Playwright Chromium). Install Edge or Chrome, set KEPLER_VISION_BROWSER to a browser executable, or run `npx playwright install chromium`.",
    );
  }
  if (/Cannot find (module|package)|ERR_MODULE_NOT_FOUND/i.test(msg)) {
    return new DriverError("playwright_unavailable", "The `playwright` package is not installed in the worker.");
  }
  return new DriverError("browser_error", msg.slice(0, 200));
}

// Minimal structural types so the worker compiles without Playwright's own typings.
interface PwRequest {
  url(): string;
}
interface PwRoute {
  request(): PwRequest;
  continue(): Promise<void>;
  abort(): Promise<void>;
}
interface PwResponse {
  url(): string;
  status(): number;
}
interface PwApiResponse {
  status(): number;
}
interface PwPage {
  on(event: string, cb: (arg: unknown) => void): void;
  goto(url: string, o: { waitUntil: string; timeout: number }): Promise<unknown>;
  evaluate<T>(fn: () => T): Promise<T>;
  screenshot(o: { path: string; fullPage: boolean }): Promise<unknown>;
  url(): string;
}
interface PwContext {
  route(pattern: string, h: (r: PwRoute) => Promise<void>): Promise<void>;
  newPage(): Promise<PwPage>;
  close(): Promise<void>;
  request: { get(url: string, o: { timeout: number; maxRedirects: number }): Promise<PwApiResponse> };
}
interface PwBrowser {
  newContext(o: Record<string, unknown>): Promise<PwContext>;
  close(): Promise<void>;
}
interface PwModule {
  chromium: { launch(o: { headless: boolean; channel?: string; executablePath?: string }): Promise<PwBrowser> };
}

const MAX_LINKS = 20;

/** Same-origin links only (never leaves the allowlist); GET with a short timeout, body discarded. */
async function brokenLinks(ctx: PwContext, pageUrl: string, links: string[], allowed: readonly string[]): Promise<string[]> {
  const origin = new URL(pageUrl).origin;
  const seen = new Set<string>();
  const broken: string[] = [];
  for (const raw of links) {
    let u: URL;
    try {
      u = new URL(raw, pageUrl);
    } catch {
      continue;
    }
    u.hash = "";
    const key = u.toString();
    if (u.origin !== origin || !subRequestAllowed(key, allowed) || seen.has(key)) continue;
    seen.add(key);
    if (seen.size > MAX_LINKS) break;
    try {
      const r = await ctx.request.get(key, { timeout: 3000, maxRedirects: 3 });
      if (r.status() >= 400) broken.push(`${r.status()} ${stripQuery(u.pathname)}`);
    } catch {
      broken.push(`unreachable ${stripQuery(u.pathname)}`);
    }
  }
  return broken;
}

const MAX_SHOT_BYTES = 2 * 1024 * 1024;
const MAX_MESSAGES = 10;
const MAX_MESSAGE_CHARS = 200;

type LaunchOpts = { channel?: string; executablePath?: string };

/** Launch candidates, in order: explicit executable/channel, then installed Edge, Chrome, bundled Chromium. */
export function browserCandidates(setting: string | undefined): { label: string; opts: LaunchOpts }[] {
  const s = (setting ?? "").trim();
  if (s !== "") {
    const l = s.toLowerCase();
    if (/^(msedge|chrome|chromium|chrome-beta|msedge-beta)$/.test(l)) return [{ label: l, opts: l === "chromium" ? {} : { channel: l } }];
    return [{ label: s, opts: { executablePath: s } }];
  }
  return [
    { label: "msedge", opts: { channel: "msedge" } },
    { label: "chrome", opts: { channel: "chrome" } },
    { label: "chromium", opts: {} },
  ];
}

export function createPlaywrightDriver(allowedHosts: readonly string[]): BrowserDriver {
  let browser: PwBrowser | null = null;
  let mod: PwModule | null = null;
  let browserLabel = "";

  async function load(): Promise<PwModule> {
    if (mod !== null) return mod;
    try {
      // Imported lazily: nothing browser-related is loaded until a check is requested.
      const name = "playwright"; // variable specifier: no static dependency on the package's typings
      mod = (await import(name)) as unknown as PwModule;
      return mod;
    } catch (e) {
      throw mapLaunchError(e);
    }
  }

  async function launch(): Promise<PwBrowser> {
    if (browser !== null) return browser;
    const pw = await load();
    let last: unknown = new Error("no candidates");
    for (const c of browserCandidates(process.env["KEPLER_VISION_BROWSER"])) {
      try {
        browser = await pw.chromium.launch({ headless: true, ...c.opts });
        browserLabel = c.label;
        return browser;
      } catch (e) {
        last = e;
      }
    }
    throw mapLaunchError(last);
  }

  return {
    async ensureBrowser() {
      await launch();
      return browserLabel;
    },

    async available() {
      try {
        await load();
        return true;
      } catch {
        return false;
      }
    },

    async check(url, vp, opts) {
      const br = await launch();
      // Fresh, isolated context per viewport: no cookies or storage survive between checks.
      const ctx = await br.newContext({ viewport: { width: vp.width, height: vp.height }, javaScriptEnabled: true, acceptDownloads: false, serviceWorkers: "block" });
      try {
        await ctx.route("**/*", async (route) => {
          if (subRequestAllowed(route.request().url(), allowedHosts)) await route.continue();
          else await route.abort();
        });
        const page = await ctx.newPage();
        const consoleErrors: string[] = [];
        const pageErrors: string[] = [];
        const failed: string[] = [];
        page.on("console", (m) => {
          const msg = m as { type(): string; text(): string };
          if (msg.type() === "error" && consoleErrors.length < MAX_MESSAGES) consoleErrors.push(msg.text().slice(0, MAX_MESSAGE_CHARS));
        });
        page.on("pageerror", (e) => {
          if (pageErrors.length < MAX_MESSAGES) pageErrors.push(String((e as Error).message ?? e).slice(0, MAX_MESSAGE_CHARS));
        });
        page.on("response", (r) => {
          const res = r as PwResponse;
          if (res.status() >= 400 && failed.length < MAX_MESSAGES) failed.push(`${res.status()} ${stripQuery(res.url())}`);
        });
        page.on("requestfailed", (r) => {
          const u = stripQuery((r as PwRequest).url());
          // Chromium also reports HTTP 4xx/5xx sub-resources here; they are already listed with their status.
          if (failed.length < MAX_MESSAGES && !failed.some((f) => f.endsWith(` ${u}`))) failed.push(u);
        });
        await page.goto(url, { waitUntil: "load", timeout: 20_000 });
        const probe = await page.evaluate(domProbe);
        const broken = await brokenLinks(ctx, page.url(), probe.links, allowedHosts);
        if (broken.length > 0) probe.issues.push({ rule: "links.broken", count: broken.length, samples: broken.slice(0, 5) });
        const out: ViewportResult & { title: string } = {
          name: vp.name,
          width: vp.width,
          height: vp.height,
          title: probe.title,
          scroll_width: probe.scroll_width,
          client_width: probe.client_width,
          horizontal_overflow: probe.scroll_width > probe.client_width + 1,
          console_errors: consoleErrors,
          page_errors: pageErrors,
          failed_requests: failed,
          issues: probe.issues,
        };
        if (opts.screenshotDir !== undefined) {
          mkdirSync(opts.screenshotDir, { recursive: true });
          const file = join(opts.screenshotDir, `${vp.name}-${vp.width}x${vp.height}.png`);
          await page.screenshot({ path: file, fullPage: false });
          // Bounded: a viewport-sized PNG only, and never more than MAX_SHOT_BYTES on disk.
          if (statSync(file).size <= MAX_SHOT_BYTES) out.screenshot = file;
          else rmSync(file, { force: true });
        }
        return out;
      } finally {
        await ctx.close().catch(() => undefined);
      }
    },

    async close() {
      const b = browser;
      browser = null;
      if (b !== null) await b.close().catch(() => undefined);
    },
  };
}
