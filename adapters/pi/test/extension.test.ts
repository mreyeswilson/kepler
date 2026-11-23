import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import keplerExtension from "../extensions/kepler.ts";
import { resolveEndpoint } from "../src/endpoint.ts";
import { startMock, wellBehaved } from "./helpers.ts";

type Handler = (event: unknown, ctx: unknown) => unknown;

function load(env: Record<string, string>): Map<string, Handler> {
  const handlers = new Map<string, Handler>();
  const saved: Record<string, string | undefined> = {};
  for (const [k, v] of Object.entries(env)) {
    saved[k] = process.env[k];
    process.env[k] = v;
  }
  try {
    keplerExtension({ on: (e: string, h: Handler) => void handlers.set(e, h) } as never);
  } finally {
    for (const [k, v] of Object.entries(saved)) {
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
    }
  }
  return handlers;
}

const ctx = { cwd: "/work/repo", sessionManager: { getSessionId: () => "sess-9" } };

test("registers the automatic hooks", () => {
  const h = load({});
  assert.deepEqual([...h.keys()].sort(), ["agent_settled", "agent_start", "before_agent_start", "context", "input", "session_shutdown", "tool_call", "tool_result"]);
});

test("input hook is observe-only: returns undefined immediately even with no core", () => {
  const h = load({ KEPLER_IPC_NAME: "nocore-test" });
  const event = { type: "input", text: "do not touch me", source: "interactive" };
  const snapshot = JSON.stringify(event);
  const result = h.get("input")?.(event, ctx);
  assert.equal(result, undefined); // Pi treats undefined as { action: "continue" }
  assert.equal(JSON.stringify(event), snapshot);
});

test("input hook survives hostile contexts and events without throwing", () => {
  const h = load({});
  const input = h.get("input");
  assert.ok(input);
  assert.doesNotThrow(() => input({ type: "input", text: 5, source: null }, {}));
  assert.doesNotThrow(() => input(null as never, undefined as never));
  assert.doesNotThrow(() => h.get("session_shutdown")?.({}, ctx));
});

test("input hook forwards to a live core (endpoint from env) without altering the input", async () => {
  const runtime = mkdtempSync(join(tmpdir(), "kpl-x-"));
  const env = { KEPLER_IPC_NAME: `t${process.pid}`, XDG_RUNTIME_DIR: runtime };
  if (process.platform !== "win32") mkdirSync(join(runtime, "kepler"));
  const endpoint = resolveEndpoint({ env: { ...process.env, ...env } });
  const m = await startMock(wellBehaved, endpoint);
  try {
    const h = load(env);
    const event = { type: "input", text: "hello core", source: "interactive" };
    assert.equal(h.get("input")?.(event, ctx), undefined);
    for (let i = 0; i < 100 && m.received.length < 2; i++) await new Promise((r) => setTimeout(r, 20));
    const sent = JSON.parse(m.received[1] ?? "{}") as { event?: { payload?: { text?: string }; session_id?: string; harness?: string } };
    assert.equal(sent.event?.payload?.text, "hello core");
    assert.equal(sent.event?.session_id, "sess-9");
    assert.equal(sent.event?.harness, "pi");
    h.get("session_shutdown")?.({}, ctx);
  } finally {
    await m.close();
    rmSync(runtime, { recursive: true, force: true });
  }
});
