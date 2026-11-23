import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { stripQuery, subRequestAllowed } from "./allowlist.ts";
import { domProbe } from "./probe.ts";
import type { Viewport, ViewportResult } from "./protocol.ts";

/** What the worker needs from a browser. The real one is Playwright; tests inject a fake. */
export interface BrowserDriver {
  /** Is the browser automation package importable? (Does not launch anything.) */
  available(): Promise<boolean>;
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
  if (/Executable doesn't exist|playwright install|browserType\.launch/i.test(msg)) {
    return new DriverError("browser_unavailable", "Chromium is not installed for Playwright (run `npx playwright install chromium`).");
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
}
interface PwBrowser {
  newContext(o: Record<string, unknown>): Promise<PwContext>;
  close(): Promise<void>;
}
interface PwModule {
  chromium: { launch(o: { headless: boolean }): Promise<PwBrowser> };
}

const MAX_MESSAGES = 10;
const MAX_MESSAGE_CHARS = 200;

export function createPlaywrightDriver(allowedHosts: readonly string[]): BrowserDriver {
  let browser: PwBrowser | null = null;
  let mod: PwModule | null = null;

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

  return {
    async available() {
      try {
        await load();
        return true;
      } catch {
        return false;
      }
    },

    async check(url, vp, opts) {
      const pw = await load();
      if (browser === null) {
        try {
          browser = await pw.chromium.launch({ headless: true });
        } catch (e) {
          throw mapLaunchError(e);
        }
      }
      // Fresh, isolated context per viewport: no cookies or storage survive between checks.
      const ctx = await browser.newContext({ viewport: { width: vp.width, height: vp.height }, javaScriptEnabled: true, acceptDownloads: false, serviceWorkers: "block" });
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
        page.on("requestfailed", (r) => {
          if (failed.length < MAX_MESSAGES) failed.push(stripQuery((r as PwRequest).url()));
        });
        await page.goto(url, { waitUntil: "load", timeout: 20_000 });
        const probe = await page.evaluate(domProbe);
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
          out.screenshot = file;
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
