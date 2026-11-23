import { randomBytes } from "node:crypto";
import { mkdtempSync, rmSync } from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

export interface MockConn {
  socket: net.Socket;
  /** Write a raw string (add your own newline). */
  raw(s: string): void;
  /** Write one JSON frame followed by a newline. */
  send(frame: unknown): void;
}

export interface Mock {
  endpoint: string;
  received: string[];
  close(): Promise<void>;
}

/** Start a mock Kepler core on a temporary named pipe / Unix socket. `onLine` sees each client line. */
export async function startMock(
  onLine: (line: string, c: MockConn, state: { n: number }) => void,
  fixedEndpoint?: string,
): Promise<Mock> {
  let dir: string | null = null;
  let endpoint: string;
  if (fixedEndpoint !== undefined) {
    endpoint = fixedEndpoint;
  } else if (process.platform === "win32") {
    endpoint = `\\\\.\\pipe\\kepler-test-${randomBytes(6).toString("hex")}`;
  } else {
    dir = mkdtempSync(join(tmpdir(), "kpl-"));
    endpoint = join(dir, "s.sock");
  }
  const received: string[] = [];
  const sockets = new Set<net.Socket>();
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    socket.on("error", () => undefined);
    const c: MockConn = {
      socket,
      raw: (s) => void socket.write(s),
      send: (f) => void socket.write(`${JSON.stringify(f)}\n`),
    };
    const state = { n: 0 };
    let buf = "";
    socket.on("data", (d) => {
      buf += d.toString("utf8");
      let i: number;
      while ((i = buf.indexOf("\n")) >= 0) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        received.push(line);
        state.n += 1;
        onLine(line, c, state);
      }
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(endpoint, resolve);
  });
  return {
    endpoint,
    received,
    close: () =>
      new Promise<void>((resolve) => {
        for (const s of sockets) s.destroy();
        server.close(() => {
          if (dir) rmSync(dir, { recursive: true, force: true });
          resolve();
        });
      }),
  };
}

export const HELLO_ACK = { type: "hello_ack", protocol: 1, server_version: "test", max_frame_bytes: 131072, negotiated: null };

/** A well-behaved core: acks hello, answers every event with a passthrough decision. */
export function wellBehaved(line: string, c: MockConn): void {
  const f = JSON.parse(line) as { type: string; request_id?: string; event?: { id: string } };
  if (f.type === "hello") c.send(HELLO_ACK);
  else if (f.type === "event") {
    c.send({ type: "decision", request_id: f.request_id, event_id: f.event?.id, decision: { action: "passthrough", reason: "test" } });
  }
}
