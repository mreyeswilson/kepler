import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";
import { checkUrl, stripQuery } from "./allowlist.ts";
import { DriverError, type BrowserDriver } from "./driver.ts";
import {
  encode,
  errResponse,
  okResponse,
  parseRequest,
  PROTOCOL_VERSION,
  type CheckResult,
  type Response,
  type ViewportResult,
} from "./protocol.ts";

export interface WorkerOptions {
  input: Readable;
  output: Writable;
  driver: BrowserDriver;
  allowedHosts?: readonly string[];
  /** Refuse work (and exit) when the resident set exceeds this many bytes. 0 disables. */
  rssLimitBytes?: number;
  /** Close the browser and stop after this long without a request. 0 disables. */
  idleMs?: number;
  rss?: () => number;
}

function clipList(list: string[]): string[] {
  return list.slice(0, 10).map((s) => s.slice(0, 200));
}

/** Last line of defense before data leaves the worker: bounded, URL queries removed. */
export function sanitizeViewport(v: ViewportResult): ViewportResult {
  const out: ViewportResult = {
    name: v.name,
    width: v.width,
    height: v.height,
    scroll_width: v.scroll_width,
    client_width: v.client_width,
    horizontal_overflow: v.horizontal_overflow,
    console_errors: clipList(v.console_errors),
    page_errors: clipList(v.page_errors),
    failed_requests: clipList(v.failed_requests.map(stripQuery)),
    issues: v.issues.slice(0, 20).map((i) => ({ rule: i.rule.slice(0, 64), count: i.count, samples: clipList(i.samples).slice(0, 5) })),
  };
  if (v.screenshot !== undefined) out.screenshot = v.screenshot;
  return out;
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => reject(new DriverError("timeout", `check exceeded ${ms} ms`)), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e: unknown) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}

/** Run the protocol loop until stdin closes, a `shutdown` request, idle expiry or the RSS guard. */
export function runWorker(opts: WorkerOptions): Promise<void> {
  const allowed = opts.allowedHosts ?? [];
  const rss = opts.rss ?? (() => process.memoryUsage().rss);
  const rl = createInterface({ input: opts.input, crlfDelay: Infinity });
  let chain: Promise<void> = Promise.resolve();
  let stopped = false;
  let idleTimer: NodeJS.Timeout | undefined;

  return new Promise<void>((done) => {
    const send = (r: Response): void => {
      opts.output.write(encode(r) + "\n");
    };
    const stop = async (): Promise<void> => {
      if (stopped) return;
      stopped = true;
      if (idleTimer !== undefined) clearTimeout(idleTimer);
      await opts.driver.close();
      rl.close();
      done();
    };
    const arm = (): void => {
      if (idleTimer !== undefined) clearTimeout(idleTimer);
      if ((opts.idleMs ?? 0) > 0) idleTimer = setTimeout(() => void chain.then(stop), opts.idleMs);
    };
    arm();

    const handle = async (line: string): Promise<void> => {
      if (stopped || line.trim() === "") return;
      arm();
      const parsed = parseRequest(line);
      if (!parsed.ok) {
        send(errResponse(parsed.id, parsed.code, parsed.message));
        return;
      }
      const req = parsed.req;
      if (req.op === "shutdown") {
        send(okResponse(req.id, { bye: true }));
        await stop();
        return;
      }
      if (req.op === "hello") {
        send(okResponse(req.id, { worker: "kepler-vision", protocol: PROTOCOL_VERSION, node: process.version, playwright: await opts.driver.available() }));
        return;
      }
      if ((opts.rssLimitBytes ?? 0) > 0 && rss() > (opts.rssLimitBytes ?? 0)) {
        send(errResponse(req.id, "memory_limit", "worker memory limit exceeded; restarting is required"));
        await stop();
        return;
      }
      const url = checkUrl(req.url, allowed);
      if (!url.ok) {
        send(errResponse(req.id, "url_not_allowed", url.reason));
        return;
      }
      try {
        const results: ViewportResult[] = [];
        let title = "";
        const deadline = Date.now() + req.timeout_ms;
        for (const vp of req.viewports) {
          const left = deadline - Date.now();
          if (left <= 0) throw new DriverError("timeout", `check exceeded ${req.timeout_ms} ms`);
          const shot = req.screenshot_dir === undefined ? {} : { screenshotDir: req.screenshot_dir };
          const r = await withTimeout(opts.driver.check(url.url.toString(), vp, shot), left);
          title = r.title;
          results.push(sanitizeViewport(r));
        }
        const result: CheckResult = { url: stripQuery(url.url.toString()), title: title.slice(0, 120), viewports: results };
        send(okResponse(req.id, result));
      } catch (e) {
        // A timed-out page may still be running: drop the whole browser.
        await opts.driver.close();
        const code = e instanceof DriverError ? e.code : "browser_error";
        send(errResponse(req.id, code, e instanceof Error ? e.message : String(e)));
      }
    };

    rl.on("line", (line) => {
      chain = chain.then(() => handle(line)).catch(() => undefined);
    });
    rl.on("close", () => void chain.then(stop));
  });
}
