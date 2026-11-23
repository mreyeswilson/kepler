import assert from "node:assert/strict";
import { test } from "node:test";
import { KeplerClient } from "../src/client.ts";
import { validateEnvelope } from "../src/envelope.ts";
import { buildInputReceived, InputObserver, projectIdFromCwd, sanitizeIdentifier } from "../src/observer.ts";
import { startMock, wellBehaved } from "./helpers.ts";

const fixed = { now: () => new Date("2026-01-02T03:04:05.123Z"), newId: () => "abc" };
const obs = { text: "Fix the failing test in parser.rs", source: "interactive", cwd: "/work/repo", sessionId: "sess-001" };

test("maps a Pi input to a valid input.received v1 envelope with verbatim text", () => {
  const r = buildInputReceived(obs, fixed);
  assert.equal(r.ok, true);
  if (!r.ok) return;
  assert.equal(r.event.version, 1);
  assert.equal(r.event.type, "input.received");
  assert.equal(r.event.harness, "pi");
  assert.equal(r.event.timestamp, "2026-01-02T03:04:05.123Z");
  assert.equal(r.event.session_id, "sess-001");
  assert.equal(r.event.id, "pi-abc");
  assert.deepEqual(r.event.payload, { text: obs.text });
  assert.equal(validateEnvelope(r.event), null);
  // the filesystem path is never sent; only a pseudonymous project id
  assert.match(r.event.project_id, /^pi-[0-9a-f]{16}$/);
  assert.ok(!JSON.stringify(r.event).includes("/work/repo"));
  assert.equal(r.event.project_id, projectIdFromCwd("/work/repo"));
});

test("text is preserved byte for byte (unicode, whitespace, control chars)", () => {
  const text = "  línea 1\r\n\t日本語 🚀 \u0007 trailing  ";
  const r = buildInputReceived({ ...obs, text }, fixed);
  assert.equal(r.ok && r.event.payload.text, text);
});

test("session ids are sanitized into valid identifiers", () => {
  assert.equal(sanitizeIdentifier("../../etc/passwd", "x"), ".._.._etc_passwd");
  assert.equal(sanitizeIdentifier("", "fallback"), "fallback");
  assert.equal(sanitizeIdentifier("a".repeat(300), "x").length, 128);
  const r = buildInputReceived({ ...obs, sessionId: "a b/c" }, fixed);
  assert.equal(r.ok && r.event.session_id, "a_b_c");
});

test("events that cannot be forwarded verbatim are skipped, never altered", () => {
  const skip = (o: Partial<typeof obs>) => {
    const r = buildInputReceived({ ...obs, ...o }, fixed);
    return r.ok ? "sent" : r.skip;
  };
  assert.equal(skip({ source: "extension" }), "foreign_source");
  assert.equal(skip({ source: "weird" }), "foreign_source");
  assert.equal(skip({ text: "" }), "empty");
  assert.equal(skip({ text: "x".repeat(16 * 1024 + 1) }), "oversized");
  assert.equal(skip({ text: "é".repeat(8 * 1024 + 1) }), "oversized");
  assert.equal(skip({ text: "a\0b" }), "contains_nul");
  assert.equal(skip({ text: "x".repeat(16 * 1024) }), "sent");
  assert.equal(skip({ source: "rpc" }), "sent");
});

test("observer: core down => not forwarded, no throw", async () => {
  const m = await startMock(wellBehaved);
  const endpoint = m.endpoint;
  await m.close();
  const o = new InputObserver(new KeplerClient({ endpoint, connectTimeoutMs: 100 }), fixed);
  assert.deepEqual(await o.observeInput(obs), { forwarded: false, why: "unavailable" });
});

test("observer: forwards to a live mock core and reports the decision only", async () => {
  const m = await startMock(wellBehaved);
  try {
    const client = new KeplerClient({ endpoint: m.endpoint, requestTimeoutMs: 300, handshakeTimeoutMs: 300 });
    const o = new InputObserver(client, fixed);
    assert.deepEqual(await o.observeInput(obs), { forwarded: true, decision: "passthrough" });
    const sent = JSON.parse(m.received[1] ?? "") as { event: { payload: { text: string } } };
    assert.equal(sent.event.payload.text, obs.text);
    o.close();
  } finally {
    await m.close();
  }
});

test("observer: a throwing client is contained", async () => {
  const o = new InputObserver({
    sendEvent: () => {
      throw new Error("boom");
    },
    close: () => {
      throw new Error("boom");
    },
  });
  assert.deepEqual(await o.observeInput(obs), { forwarded: false, why: "closed" });
  o.close();
});
