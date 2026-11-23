import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { KeplerClient } from "../src/client.ts";
import { validateEnvelope, validateEnvelopeText } from "../src/envelope.ts";
import { buildInputReceived } from "../src/observer.ts";
import { startMock, wellBehaved } from "./helpers.ts";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "fixtures", "events");
const read = (rel: string): string => readFileSync(join(dir, rel), "utf8");
const manifest = JSON.parse(read("manifest.json")) as { valid: string[]; invalid: { file: string; error: string }[] };

test("every shared valid fixture passes the TS validator", () => {
  assert.ok(manifest.valid.length >= 11);
  for (const f of manifest.valid) assert.equal(validateEnvelopeText(read(f)), null, f);
});

test("every shared invalid fixture is rejected with the manifest error code", () => {
  assert.ok(manifest.invalid.length >= 14);
  for (const { file, error } of manifest.invalid) {
    const e = validateEnvelopeText(read(file));
    assert.ok(e, `${file} must be rejected`);
    assert.equal(e.code, error, file);
  }
});

test("adapter output has exactly the shape of the shared input.received fixture", () => {
  const fx = JSON.parse(read("valid/input.received.json")) as Record<string, unknown>;
  const r = buildInputReceived(
    { text: "Fix the failing test in parser.rs", source: "interactive", cwd: "/x", sessionId: "sess-001" },
    { now: () => new Date("2026-01-02T03:04:05.123Z"), newId: () => "1" },
  );
  assert.ok(r.ok);
  assert.deepEqual(Object.keys(r.event).sort(), Object.keys(fx).sort());
  assert.equal(r.event.version, fx["version"]);
  assert.equal(r.event.type, fx["type"]);
  assert.equal(r.event.harness, fx["harness"]);
  assert.equal(r.event.timestamp, fx["timestamp"]);
  assert.deepEqual(r.event.payload, fx["payload"]);
});

test("valid fixtures round-trip through the client unchanged", async () => {
  const m = await startMock(wellBehaved);
  try {
    const c = new KeplerClient({ endpoint: m.endpoint, requestTimeoutMs: 500, handshakeTimeoutMs: 500 });
    for (const f of manifest.valid) {
      const ev = JSON.parse(read(f)) as unknown;
      const r = await c.sendEvent(ev);
      assert.equal(r.ok, true, f);
      const last = JSON.parse(m.received[m.received.length - 1] ?? "") as { event: unknown };
      assert.deepEqual(last.event, ev, f);
    }
    c.close();
  } finally {
    await m.close();
  }
});

test("envelope validator rejects non-object and oversized envelopes", () => {
  assert.equal(validateEnvelope(null)?.code, "malformed");
  const ev = JSON.parse(read("valid/input.received.json")) as { payload: { text: string } };
  ev.payload.text = "x".repeat(16 * 1024 + 1);
  assert.equal(validateEnvelope(ev)?.code, "field_too_large");
});
