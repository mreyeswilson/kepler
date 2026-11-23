import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { PassThrough } from "node:stream";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { checkUrl, parseAllowedHosts, stripQuery, subRequestAllowed } from "../src/allowlist.ts";
import { DriverError, mapLaunchError, type BrowserDriver } from "../src/driver.ts";
import { domProbe } from "../src/probe.ts";
import { encode, errResponse, MAX_RESPONSE_BYTES, parseRequest } from "../src/protocol.ts";
import { runWorker, sanitizeViewport } from "../src/worker.ts";

const VP = { name: "mobile", width: 375, height: 812 };

test("allowlist: local hosts only by default", () => {
  for (const ok of ["http://localhost:3000/a?x=1", "https://127.0.0.1/", "http://[::1]:8080/", "http://app.localhost/"]) {
    assert.equal(checkUrl(ok).ok, true, ok);
  }
  for (const bad of [
    "https://example.com/",
    "http://localhost.evil.com/",
    "http://127.0.0.1.evil.com/",
    "http://user:pw@localhost/",
    "http://localhost@evil.com/",
    "file:///etc/passwd",
    "javascript:alert(1)",
    "ftp://localhost/",
    "not a url",
    "http://192.168.1.10/",
    "http://169.254.169.254/latest/meta-data",
  ]) {
    assert.equal(checkUrl(bad).ok, false, bad);
  }
});

test("allowlist: extra hosts are explicit and wildcard is suffix-only", () => {
  const extra = parseAllowedHosts("staging.example.com, *.dev.example.org, bad host!, ");
  assert.deepEqual(extra, ["staging.example.com", "*.dev.example.org"]);
  assert.equal(checkUrl("https://staging.example.com/", extra).ok, true);
  assert.equal(checkUrl("https://a.dev.example.org/", extra).ok, true);
  assert.equal(checkUrl("https://dev.example.org/", extra).ok, false);
  assert.equal(checkUrl("https://evilstaging.example.com/", extra).ok, false);
  assert.equal(subRequestAllowed("data:image/png;base64,AAAA"), true);
  assert.equal(subRequestAllowed("https://cdn.example.com/x.js"), false);
  assert.equal(stripQuery("http://localhost/a?token=abc#frag"), "http://localhost/a");
});

test("protocol: strict validation of requests", () => {
  const ok = parseRequest(JSON.stringify({ v: 1, id: "r1", op: "check", url: "http://localhost/", viewports: [VP], timeout_ms: 5000 }));
  assert.equal(ok.ok, true);
  const bad: [unknown, string][] = [
    [{ v: 2, id: "r", op: "hello" }, "unsupported_version"],
    [{ v: 1, id: "r", op: "nope" }, "unknown_op"],
    [{ v: 1, op: "hello" }, "malformed"],
    [{ v: 1, id: "r", op: "check", url: "http://localhost/", viewports: [], timeout_ms: 5000 }, "invalid_request"],
    [{ v: 1, id: "r", op: "check", url: "http://localhost/", viewports: [{ ...VP, width: 10 }], timeout_ms: 5000 }, "invalid_request"],
    [{ v: 1, id: "r", op: "check", url: "http://localhost/", viewports: [VP], timeout_ms: 5 }, "invalid_request"],
    [{ v: 1, id: "r", op: "check", url: "x".repeat(3000), viewports: [VP] }, "invalid_request"],
  ];
  for (const [req, code] of bad) {
    const r = parseRequest(JSON.stringify(req));
    assert.equal(r.ok, false);
    if (!r.ok) assert.equal(r.code, code);
  }
  assert.equal(parseRequest("{not json").ok, false);
  assert.equal(parseRequest("x".repeat(70_000)).ok, false);
  assert.equal(parseRequest("[]").ok, false);
});

test("protocol: oversized responses are replaced by an error", () => {
  const huge = encode({ v: 1, id: "r", ok: true, result: "x".repeat(MAX_RESPONSE_BYTES + 10) });
  assert.equal(JSON.parse(huge).error.code, "result_too_large");
  assert.equal(JSON.parse(encode(errResponse("r", "c", "m".repeat(1000)))).error.message.length, 300);
});

function fakeDriver(over: Partial<BrowserDriver> = {}): BrowserDriver & { calls: string[]; closed: number } {
  const state = { calls: [] as string[], closed: 0 };
  return {
    get calls() {
      return state.calls;
    },
    get closed() {
      return state.closed;
    },
    available: async () => true,
    check: async (url, vp) => {
      state.calls.push(url);
      return {
        name: vp.name,
        width: vp.width,
        height: vp.height,
        title: "Demo",
        scroll_width: vp.width + 40,
        client_width: vp.width,
        horizontal_overflow: true,
        console_errors: ["boom ".repeat(100)],
        page_errors: [],
        failed_requests: ["http://localhost/x.js?token=SECRET#a"],
        issues: [{ rule: "a11y.img_alt", count: 2, samples: ["img.hero"] }],
      };
    },
    close: async () => {
      state.closed++;
    },
    ...over,
  };
}

async function converse(driver: BrowserDriver, lines: unknown[], extra: Partial<Parameters<typeof runWorker>[0]> = {}) {
  const input = new PassThrough();
  const output = new PassThrough();
  const chunks: string[] = [];
  output.on("data", (c: Buffer) => chunks.push(c.toString("utf8")));
  const done = runWorker({ input, output, driver, ...extra });
  for (const l of lines) input.write((typeof l === "string" ? l : JSON.stringify(l)) + "\n");
  input.end();
  await done;
  return chunks.join("").trim().split("\n").filter((l) => l !== "").map((l) => JSON.parse(l));
}

test("worker: hello, check with sanitization, and clean shutdown", async () => {
  const d = fakeDriver();
  const out = await converse(d, [
    { v: 1, id: "h", op: "hello" },
    { v: 1, id: "c", op: "check", url: "http://localhost:3000/p?secret=1", viewports: [VP], timeout_ms: 5000 },
    { v: 1, id: "s", op: "shutdown" },
  ]);
  assert.equal(out[0].result.playwright, true);
  assert.equal(out[1].ok, true);
  const vp = out[1].result.viewports[0];
  assert.equal(out[1].result.url, "http://localhost:3000/p", "query string must not be echoed");
  assert.equal(vp.horizontal_overflow, true);
  assert.equal(vp.failed_requests[0], "http://localhost/x.js", "query and fragment stripped");
  assert.ok(vp.console_errors[0].length <= 200);
  assert.deepEqual(out[2], { v: 1, id: "s", ok: true, result: { bye: true } });
  assert.ok(d.closed >= 1, "browser closed on shutdown");
});

test("worker: disallowed URLs never reach the driver", async () => {
  const d = fakeDriver();
  const out = await converse(d, [{ v: 1, id: "c", op: "check", url: "https://example.com/", viewports: [VP], timeout_ms: 5000 }]);
  assert.equal(out[0].error.code, "url_not_allowed");
  assert.deepEqual(d.calls, []);
  // Explicitly allowed host works.
  const d2 = fakeDriver();
  const ok = await converse(d2, [{ v: 1, id: "c", op: "check", url: "https://staging.example.com/", viewports: [VP], timeout_ms: 5000 }], { allowedHosts: ["staging.example.com"] });
  assert.equal(ok[0].ok, true);
});

test("worker: a hung check times out and the browser is dropped", async () => {
  const d = fakeDriver({ check: () => new Promise(() => undefined) });
  const t0 = Date.now();
  const out = await converse(d, [{ v: 1, id: "c", op: "check", url: "http://localhost/", viewports: [VP], timeout_ms: 1000 }]);
  assert.equal(out[0].error.code, "timeout");
  assert.ok(Date.now() - t0 < 5000);
  assert.ok(d.closed >= 1);
});

test("worker: driver errors map to stable codes; garbage lines get error frames and do not stop the loop", async () => {
  const d = fakeDriver({
    check: async () => {
      throw new DriverError("browser_unavailable", "no chromium");
    },
  });
  const out = await converse(d, ["{broken", { v: 1, id: "c", op: "check", url: "http://localhost/", viewports: [VP], timeout_ms: 2000 }, { v: 1, id: "h", op: "hello" }]);
  assert.equal(out[0].error.code, "malformed");
  assert.equal(out[1].error.code, "browser_unavailable");
  assert.equal(out[2].ok, true);
});

test("worker: memory guard refuses work and stops", async () => {
  const d = fakeDriver();
  const out = await converse(
    d,
    [
      { v: 1, id: "c", op: "check", url: "http://localhost/", viewports: [VP], timeout_ms: 2000 },
      { v: 1, id: "d", op: "hello" },
    ],
    { rssLimitBytes: 1000, rss: () => 5000 },
  );
  assert.equal(out[0].error.code, "memory_limit");
  assert.equal(out.length, 1, "no further requests are served after the guard trips");
  assert.deepEqual(d.calls, []);
});

test("worker: idle expiry closes the browser and ends the loop without input closing", async () => {
  const d = fakeDriver();
  const input = new PassThrough();
  const output = new PassThrough();
  const t0 = Date.now();
  await runWorker({ input, output, driver: d, idleMs: 80 });
  assert.ok(Date.now() - t0 < 3000);
  assert.ok(d.closed >= 1);
});

test("sanitizeViewport bounds lists and strips URL queries", () => {
  const v = sanitizeViewport({
    ...VP,
    scroll_width: 1,
    client_width: 1,
    horizontal_overflow: false,
    console_errors: Array.from({ length: 50 }, () => "e"),
    page_errors: [],
    failed_requests: ["http://localhost/a?x=1"],
    issues: Array.from({ length: 50 }, (_, i) => ({ rule: `r${i}`, count: 1, samples: Array.from({ length: 20 }, () => "s") })),
  });
  assert.equal(v.console_errors.length, 10);
  assert.equal(v.issues.length, 20);
  assert.equal(v.issues[0]?.samples.length, 5);
  assert.equal(v.failed_requests[0], "http://localhost/a");
});

test("mapLaunchError recognizes a missing browser and a missing package", () => {
  assert.equal(mapLaunchError(new Error("browserType.launch: Executable doesn't exist at C:\\x")).code, "browser_unavailable");
  assert.equal(mapLaunchError(new Error("Cannot find package 'playwright' imported from x")).code, "playwright_unavailable");
  assert.equal(mapLaunchError("weird").code, "browser_error");
});

test("domProbe is self-contained (no module-scope captures) and serializable", () => {
  // Playwright serializes the function source; it must not reference anything outside itself.
  const src = domProbe.toString();
  assert.ok(src.startsWith("function") || src.includes("=>"));
  assert.ok(!/\b(import|require|DriverError|stripQuery)\b/.test(src));
});

test("main.ts as a real process: hello answers, nothing launches, shutdown exits 0", async () => {
  const main = fileURLToPath(new URL("../src/main.ts", import.meta.url));
  const child = spawn(process.execPath, [main], { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, KEPLER_VISION_IDLE_MS: "0" } });
  let out = "";
  child.stdout.on("data", (c: Buffer) => (out += c.toString("utf8")));
  child.stdin.write(JSON.stringify({ v: 1, id: "h", op: "hello" }) + "\n");
  child.stdin.write(JSON.stringify({ v: 1, id: "x", op: "check", url: "https://example.com", viewports: [VP], timeout_ms: 2000 }) + "\n");
  child.stdin.write(JSON.stringify({ v: 1, id: "s", op: "shutdown" }) + "\n");
  const code: number | null = await new Promise((resolve) => child.on("close", resolve));
  assert.equal(code, 0);
  const frames = out.trim().split("\n").map((l) => JSON.parse(l));
  assert.equal(frames[0].result.worker, "kepler-vision");
  assert.equal(frames[1].error.code, "url_not_allowed");
  assert.equal(frames[2].ok, true);
});
