import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { test } from "node:test";
import { KeplerClient } from "../src/client.ts";
import { HELLO_ACK, startMock, wellBehaved, type Mock } from "./helpers.ts";

const EVENT = {
  version: 1,
  id: "e1",
  type: "input.received",
  timestamp: "2026-01-02T03:04:05Z",
  harness: "pi",
  project_id: "p",
  session_id: "s",
  correlation_id: "c",
  payload: { text: "fix the bug" },
};
const fast = { connectTimeoutMs: 200, handshakeTimeoutMs: 200, requestTimeoutMs: 200 };

async function withMock(handler: Parameters<typeof startMock>[0], fn: (m: Mock) => Promise<void>): Promise<void> {
  const m = await startMock(handler);
  try {
    await fn(m);
  } finally {
    await m.close();
  }
}

const outcome = (r: Awaited<ReturnType<KeplerClient["sendEvent"]>>): [boolean, string] => [r.ok, r.ok ? "" : r.reason];

test("handshake then decision; hello frame matches ADR-0007", async () => {
  await withMock(wellBehaved, async (m) => {
    const c = new KeplerClient({ endpoint: m.endpoint, clientVersion: "9.9.9", ...fast });
    const r = await c.sendEvent(EVENT);
    assert.equal(r.ok, true);
    if (r.ok) {
      assert.equal(r.decision.action, "passthrough");
      assert.equal(r.eventId, "e1");
    }
    assert.deepEqual(JSON.parse(m.received[0] ?? ""), { type: "hello", protocol: 1, client: "pi-adapter", client_version: "9.9.9" });
    const ev = JSON.parse(m.received[1] ?? "");
    assert.equal(ev.type, "event");
    assert.equal(ev.request_id, "r1");
    assert.deepEqual(ev.event, EVENT);
    // connection is reused: second event, no second hello
    assert.equal((await c.sendEvent(EVENT)).ok, true);
    assert.equal(m.received.filter((l) => l.includes('"hello"')).length, 1);
    c.close();
  });
});

test("core absent => passthrough (unavailable), never throws", async () => {
  const m = await startMock(wellBehaved);
  const endpoint = m.endpoint;
  await m.close();
  const r = await new KeplerClient({ endpoint, ...fast }).sendEvent(EVENT);
  assert.deepEqual(outcome(r), [false, "unavailable"]);
});

test("handshake timeout => passthrough", async () => {
  await withMock(
    () => undefined,
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "timeout"]);
    },
  );
});

test("request timeout => passthrough, and next call reconnects", async () => {
  let silent = true;
  await withMock(
    (line, c) => {
      const f = JSON.parse(line) as { type: string };
      if (f.type === "hello") c.send(HELLO_ACK);
      else if (!silent) wellBehaved(line, c);
    },
    async (m) => {
      const c = new KeplerClient({ endpoint: m.endpoint, ...fast });
      assert.deepEqual(outcome(await c.sendEvent(EVENT)), [false, "timeout"]);
      silent = false;
      assert.equal((await c.sendEvent(EVENT)).ok, true);
      c.close();
    },
  );
});

test("oversized incoming frame (no newline) => passthrough", async () => {
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.raw("x".repeat(131072 + 10));
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "frame_too_large"]);
    },
  );
});

test("oversized incoming frame (with newline) => passthrough", async () => {
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.raw(`${"y".repeat(131073)}\n`);
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "frame_too_large"]);
    },
  );
});

test("frame of exactly the limit is accepted", async () => {
  const frame = (reason: string) =>
    JSON.stringify({ type: "decision", request_id: "r1", event_id: "e1", decision: { action: "passthrough", reason } });
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.raw(`${frame("z".repeat(131072 - frame("").length))}\n`);
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast, requestTimeoutMs: 2000 }).sendEvent(EVENT);
      assert.equal(r.ok, true);
    },
  );
});

test("outgoing frame over the limit is not sent", async () => {
  await withMock(wellBehaved, async (m) => {
    const big = { ...EVENT, payload: { text: randomBytes(70000).toString("hex") } };
    const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(big);
    assert.deepEqual(outcome(r), [false, "frame_too_large_outgoing"]);
    assert.equal(
      m.received.some((l) => l.includes('"type":"event"')),
      false,
    );
  });
});

test("fatal error frame at handshake => passthrough", async () => {
  await withMock(
    (_l, c) => {
      c.send({ type: "error", request_id: null, code: "unsupported_protocol", message: "no", fatal: true });
      c.socket.end();
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "server_error"]);
    },
  );
});

test("unexpected hello_ack protocol => passthrough", async () => {
  await withMock(
    (_l, c) => c.send({ ...HELLO_ACK, protocol: 2 }),
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "protocol"]);
    },
  );
});

test("error frame (handler_timeout) in reply => passthrough", async () => {
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.send({ type: "error", request_id: "r1", code: "handler_timeout", message: "slow", fatal: false });
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "server_error"]);
      assert.equal(r.ok ? "" : r.detail, "handler_timeout");
    },
  );
});

test("malformed frame => passthrough", async () => {
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.raw("not json\n");
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "bad_frame"]);
    },
  );
});

test("mismatched request id => passthrough", async () => {
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.send({ type: "decision", request_id: "other", event_id: "e1", decision: { action: "passthrough" } });
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "protocol"]);
    },
  );
});

test("server closes mid-request => passthrough", async () => {
  await withMock(
    (line, c) => {
      if (line.includes('"hello"')) c.send(HELLO_ACK);
      else c.socket.destroy();
    },
    async (m) => {
      const r = await new KeplerClient({ endpoint: m.endpoint, ...fast }).sendEvent(EVENT);
      assert.deepEqual(outcome(r), [false, "closed"]);
    },
  );
});

test("only one request in flight: concurrent call returns busy", async () => {
  await withMock(wellBehaved, async (m) => {
    const c = new KeplerClient({ endpoint: m.endpoint, ...fast });
    const [a, b] = await Promise.all([c.sendEvent(EVENT), c.sendEvent(EVENT)]);
    assert.equal(a.ok, true);
    assert.deepEqual(outcome(b), [false, "busy"]);
    c.close();
  });
});

test("a verdict carrying the additive `advisories` array (ADR-0014) is accepted and its decision preserved", async () => {
  const handler: Parameters<typeof startMock>[0] = (line, c) => {
    const f = JSON.parse(line) as { type: string; request_id?: string; event?: { id: string } };
    if (f.type === "hello") c.send(HELLO_ACK);
    else if (f.type === "event") {
      c.send({
        type: "decision",
        request_id: f.request_id,
        event_id: f.event?.id,
        decision: {
          action: "passthrough",
          reason: "observe_only",
          advisories: [{ source: "focus", level: "warn", code: "loop.repeated_call", message: "x" }],
        },
      });
    }
  };
  await withMock(handler, async (m) => {
    const c = new KeplerClient({ endpoint: m.endpoint, clientVersion: "9.9.9", ...fast });
    const r = await c.sendEvent(EVENT);
    assert.equal(r.ok, true);
    if (r.ok) assert.equal(r.decision.action, "passthrough");
  });
});
