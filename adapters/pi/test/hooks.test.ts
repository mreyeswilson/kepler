import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import keplerExtension from "../extensions/kepler.ts";
import { KeplerClient, type Decision, type SendResult } from "../src/client.ts";
import { resolveEndpoint } from "../src/endpoint.ts";
import { validateEnvelope } from "../src/envelope.ts";
import { PiHooks, renderDerived, type PiCtx, type Sender } from "../src/hooks.ts";
import { startMock, HELLO_ACK } from "./helpers.ts";

interface Sent {
  type: string;
  payload: Record<string, unknown>;
  session_id: string;
  id: string;
}

function fake(decide: (e: Sent) => SendResult | Promise<SendResult>): { sender: Sender; sent: Sent[] } {
  const sent: Sent[] = [];
  return {
    sent,
    sender: {
      sendEvent: async (e) => {
        assert.equal(validateEnvelope(e), null, "adapter must only send valid envelopes");
        sent.push(e as Sent);
        return decide(e as Sent);
      },
      close: () => undefined,
    },
  };
}

const pass: SendResult = { ok: true, decision: { action: "passthrough", reason: "t" }, eventId: "x" };
const decide = (d: Decision): SendResult => ({ ok: true, decision: d, eventId: "x" });
const ctx: PiCtx = { cwd: "/work/repo", sessionManager: { getSessionId: () => "sess-1" } };
const tick = () => new Promise((r) => setTimeout(r, 15));

test("enrich verdict adds a labeled block to the system prompt, never to the user prompt", async () => {
  const { sender } = fake(() =>
    decide({
      action: "enrich",
      derived: [
        { source: "forge", provenance: "inferred", text: "open question: no target" },
        { source: "memory", provenance: "recalled", text: "uses pnpm" },
      ],
    }),
  );
  const h = new PiHooks(sender);
  h.onInput({ text: "fix the bug in the login flow please", source: "interactive" }, ctx);
  const event = { prompt: "fix the bug in the login flow please", systemPrompt: "BASE" };
  const r = await h.onBeforeAgentStart(event, ctx);
  assert.ok(r);
  assert.ok(r.systemPrompt.startsWith("BASE\n\n"));
  assert.match(r.systemPrompt, /NOT user instructions/);
  assert.match(r.systemPrompt, /\[inferred\] open question: no target/);
  assert.match(r.systemPrompt, /\[recalled\] uses pnpm/);
  assert.equal(event.prompt, "fix the bug in the login flow please");
  // the pending verdict is consumed: a second start has nothing to add
  assert.equal(await h.onBeforeAgentStart(event, ctx), undefined);
});

test("no injection when disabled, on passthrough, on failure, or with nothing pending", async () => {
  const input = { text: "refactor the parser please", source: "interactive" };
  const enrich = decide({ action: "enrich", derived: [{ source: "f", provenance: "inferred", text: "x" }] });
  const off = new PiHooks(fake(() => enrich).sender, { inject: false });
  off.onInput(input, ctx);
  assert.equal(await off.onBeforeAgentStart({}, ctx), undefined);
  const p = new PiHooks(fake(() => pass).sender);
  p.onInput(input, ctx);
  assert.equal(await p.onBeforeAgentStart({}, ctx), undefined);
  const bad = new PiHooks(fake(() => ({ ok: false, reason: "unavailable" })).sender);
  bad.onInput(input, ctx);
  assert.equal(await bad.onBeforeAgentStart({}, ctx), undefined);
  assert.equal(await new PiHooks(fake(() => pass).sender).onBeforeAgentStart({}, ctx), undefined);
  const slow = new PiHooks(fake(() => new Promise(() => undefined)).sender, { verdictTimeoutMs: 30 });
  slow.onInput(input, ctx);
  const t0 = Date.now();
  assert.equal(await slow.onBeforeAgentStart({}, ctx), undefined);
  assert.ok(Date.now() - t0 < 500);
  assert.equal(renderDerived({ action: "enrich", derived: [] }), null);
  assert.equal(renderDerived({ action: "enrich", derived: [{ nope: 1 }] }), null);
});

test("tool_call blocks only on explicit deny / unapproved require_approval", async () => {
  const call = { toolName: "bash", toolCallId: "c1", input: { command: "cat .env" } };
  const deny = fake(() => decide({ action: "deny", reason: "sensitive_path" }));
  const r = await new PiHooks(deny.sender).onToolCall(call, ctx);
  assert.equal(r?.block, true);
  assert.match(r?.reason ?? "", /sensitive_path/);
  assert.equal(deny.sent[0]?.type, "tool.call.requested");
  assert.deepEqual(deny.sent[0]?.payload, { tool: "bash", command: "cat .env" });

  const appr = decide({ action: "require_approval", reason: "destructive_command" });
  const noUi = await new PiHooks(fake(() => appr).sender).onToolCall(call, ctx);
  assert.equal(noUi?.block, true, "no UI available: explicit approval verdict fails closed");
  const yes = await new PiHooks(fake(() => appr).sender).onToolCall(call, { ...ctx, hasUI: true, ui: { confirm: async () => true } });
  assert.equal(yes, undefined);
  const no = await new PiHooks(fake(() => appr).sender).onToolCall(call, { ...ctx, hasUI: true, ui: { confirm: async () => false } });
  assert.equal(no?.block, true);
  const throws = await new PiHooks(fake(() => appr).sender).onToolCall(call, {
    ...ctx,
    hasUI: true,
    ui: {
      confirm: () => {
        throw new Error("ui broke");
      },
    },
  });
  assert.equal(throws?.block, true);

  assert.equal(await new PiHooks(fake(() => pass).sender).onToolCall(call, ctx), undefined);
  // unknown future action: treated as no verdict
  assert.equal(await new PiHooks(fake(() => decide({ action: "frobnicate" })).sender).onToolCall(call, ctx), undefined);
});

test("tool_call without a verdict never blocks and never grants", async () => {
  const call = { toolName: "bash", toolCallId: "c1", input: { command: "rm -rf /" } };
  for (const reason of ["unavailable", "timeout", "busy", "protocol", "closed"] as const) {
    assert.equal(await new PiHooks(fake(() => ({ ok: false, reason })).sender).onToolCall(call, ctx), undefined, reason);
  }
  const hang = new PiHooks(fake(() => new Promise(() => undefined)).sender, { verdictTimeoutMs: 30 });
  assert.equal(await hang.onToolCall(call, ctx), undefined);
  assert.equal(await new PiHooks(fake(() => pass).sender).onToolCall(null as never, null as never), undefined);
});

test("tool_call maps paths and truncates oversized commands instead of dropping the check", async () => {
  const f = fake(() => pass);
  const h = new PiHooks(f.sender);
  await h.onToolCall({ toolName: "write", input: { path: "src/a.ts", file_path: "src/b.ts", content: "SECRET BODY" } }, ctx);
  assert.deepEqual(f.sent[0]?.payload, { tool: "write", paths: ["src/a.ts", "src/b.ts"] });
  await h.onToolCall({ toolName: "bash", input: { command: "a".repeat(10000) } }, ctx);
  assert.equal(String(f.sent[1]?.payload["command"]).length, 4096);
});

test("tool_result replaces content only on redact, and the secret is never sent back", async () => {
  const f = fake((e) =>
    String(e.payload["output_excerpt"]).includes("AKIA") ? decide({ action: "redact", reason: "secret_in_tool_result", kinds: ["aws_access_key"] }) : pass,
  );
  const h = new PiHooks(f.sender);
  await h.onToolCall({ toolName: "bash", toolCallId: "t1", input: { command: "env" } }, ctx);
  const leaked = await h.onToolResult(
    { toolName: "bash", toolCallId: "t1", isError: false, content: [{ type: "text", text: "AKIAIOSFODNN7EXAMPLE" }] },
    ctx,
  );
  assert.ok(leaked);
  assert.equal(leaked.content.length, 1);
  assert.match(leaked.content[0]?.text ?? "", /withheld.*aws_access_key/);
  assert.doesNotMatch(leaked.content[0]?.text ?? "", /AKIA/);
  const done = f.sent.at(-1);
  assert.equal(done?.type, "tool.call.completed");
  assert.equal(done?.payload["ok"], true);
  assert.equal(done?.payload["tool"], "bash");
  const clean = await h.onToolResult({ toolName: "bash", toolCallId: "t2", content: [{ type: "text", text: "3 passed" }], isError: true }, ctx);
  assert.equal(clean, undefined);
  assert.equal(f.sent.at(-1)?.payload["ok"], false);
  const failing = new PiHooks(fake(() => ({ ok: false, reason: "unavailable" })).sender);
  assert.equal(await failing.onToolResult({ toolName: "x", content: "weird" }, ctx), undefined);
});

test("agent lifecycle and context events are observe-only and labeled", async () => {
  const f = fake(() => pass);
  const h = new PiHooks(f.sender);
  h.onAgentStart(ctx);
  h.onContext({ messages: [{ role: "user", content: "x".repeat(400) }] }, ctx);
  h.onAgentSettled({ aborted: false }, ctx);
  h.onAgentSettled({ aborted: true }, ctx);
  h.onSessionShutdown("quit", ctx);
  await new Promise((r) => setTimeout(r, 120));
  const kinds = f.sent.map((e) => e.type);
  assert.deepEqual(kinds, ["agent.started", "context.prepared", "agent.settled", "agent.settled", "session.closed"]);
  assert.equal(f.sent[1]?.payload["estimated"], true);
  assert.ok(Number(f.sent[1]?.payload["tokens"]) >= 100);
  assert.equal(f.sent[2]?.payload["outcome"], "ok");
  assert.equal(f.sent[3]?.payload["outcome"], "cancelled");
  assert.doesNotThrow(() => h.onContext(undefined as never, undefined as never));
});

test("sends are serialized and bounded", async () => {
  let inflight = 0;
  let max = 0;
  const h = new PiHooks({
    sendEvent: async () => {
      inflight += 1;
      max = Math.max(max, inflight);
      await tick();
      inflight -= 1;
      return pass;
    },
    close: () => undefined,
  });
  for (let i = 0; i < 20; i++) h.onAgentStart(ctx);
  await new Promise((r) => setTimeout(r, 300));
  assert.equal(max, 1);
});

test("extension registers all eight hooks", () => {
  const names: string[] = [];
  keplerExtension({ on: (e: string) => void names.push(e) } as never);
  assert.deepEqual(names.sort(), [
    "agent_settled",
    "agent_start",
    "before_agent_start",
    "context",
    "input",
    "session_shutdown",
    "tool_call",
    "tool_result",
  ]);
});

test("through the real IPC client: a core denial blocks the tool call, silence does not", async () => {
  const runtime = mkdtempSync(join(tmpdir(), "kpl-h-"));
  const env = { ...process.env, KEPLER_IPC_NAME: `h${process.pid}`, XDG_RUNTIME_DIR: runtime };
  if (process.platform !== "win32") mkdirSync(join(runtime, "kepler"));
  const endpoint = resolveEndpoint({ env });
  const m = await startMock((line, c) => {
    const f = JSON.parse(line) as { type: string; request_id?: string; event?: { id: string; payload: { command?: string } } };
    if (f.type === "hello") c.send(HELLO_ACK);
    else if (f.type === "event") {
      const action = f.event?.payload.command === "cat .env" ? { action: "deny", reason: "sensitive_path" } : { action: "passthrough", reason: "ok" };
      c.send({ type: "decision", request_id: f.request_id, event_id: f.event?.id, decision: action });
    }
  }, endpoint);
  try {
    const client = new KeplerClient({ endpoint });
    const h = new PiHooks(client);
    const denied = await h.onToolCall({ toolName: "bash", input: { command: "cat .env" } }, ctx);
    assert.equal(denied?.block, true);
    assert.equal(await h.onToolCall({ toolName: "bash", input: { command: "cargo test" } }, ctx), undefined);
    client.close();
  } finally {
    await m.close();
  }
  // core gone: no block
  const h2 = new PiHooks(new KeplerClient({ endpoint: resolveEndpoint({ env: { ...env, KEPLER_IPC_NAME: "gone-core" } }) }));
  assert.equal(await h2.onToolCall({ toolName: "bash", input: { command: "cat .env" } }, ctx), undefined);
  rmSync(runtime, { recursive: true, force: true });
});
