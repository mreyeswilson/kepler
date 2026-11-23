import net from "node:net";

/**
 * Minimal Kepler IPC client (ADR-0007): line-delimited JSON over a per-user named pipe / Unix socket.
 *
 * Contract: `sendEvent` NEVER throws and never rejects. Any problem (core not running, timeout,
 * oversized or malformed frame, fatal error frame, unsupported protocol) resolves to `{ ok: false }`,
 * which callers must treat as "passthrough": the user's original input is never altered or lost.
 * This client never launches the core and never installs anything.
 */

export const PROTOCOL_VERSION = 1;
export const MAX_FRAME_BYTES = 131072;

export interface ClientOptions {
  /** Pipe/socket path. Resolve with `resolveEndpoint()`. */
  endpoint: string;
  clientName?: string;
  clientVersion?: string;
  connectTimeoutMs?: number;
  handshakeTimeoutMs?: number;
  requestTimeoutMs?: number;
  maxFrameBytes?: number;
}

export type FailureReason =
  | "unavailable" // core not running / cannot connect
  | "timeout"
  | "busy" // another request already in flight (we never queue)
  | "frame_too_large"
  | "frame_too_large_outgoing"
  | "bad_frame"
  | "protocol" // unexpected frame, unsupported protocol, id mismatch
  | "server_error" // error frame from the core
  | "closed";

export interface Decision {
  action: string;
  reason?: string;
  [k: string]: unknown;
}

export type SendResult =
  | { ok: true; decision: Decision; eventId: string }
  | { ok: false; reason: FailureReason; detail?: string };

class IpcFailure extends Error {
  readonly reason: FailureReason;
  constructor(reason: FailureReason, detail?: string) {
    super(detail ?? reason);
    this.reason = reason;
  }
}

interface Waiter {
  resolve: (v: unknown) => void;
  reject: (e: IpcFailure) => void;
  timer: NodeJS.Timeout;
}

/** One established connection: bounded line reader + writer. */
class Connection {
  private chunks: Buffer = Buffer.alloc(0);
  private frames: unknown[] = [];
  private failure: IpcFailure | null = null;
  private waiter: Waiter | null = null;
  private readonly socket: net.Socket;
  private readonly maxFrame: number;

  constructor(socket: net.Socket, maxFrame: number) {
    this.socket = socket;
    this.maxFrame = maxFrame;
    socket.on("data", (d: Buffer) => this.onData(d));
    socket.on("error", (e) => this.fail(new IpcFailure("closed", e.message)));
    socket.on("close", () => this.fail(new IpcFailure("closed", "connection closed")));
    socket.on("end", () => this.fail(new IpcFailure("closed", "connection ended")));
  }

  get alive(): boolean {
    return this.failure === null && !this.socket.destroyed;
  }

  private onData(data: Buffer): void {
    if (this.failure) return;
    this.chunks = this.chunks.length === 0 ? data : Buffer.concat([this.chunks, data]);
    for (;;) {
      const nl = this.chunks.indexOf(0x0a);
      if (nl < 0) {
        // No newline yet: more than maxFrame bytes buffered means the frame is already oversized.
        if (this.chunks.length > this.maxFrame) this.fail(new IpcFailure("frame_too_large", "incoming frame exceeds limit"));
        return;
      }
      let end = nl;
      if (end > 0 && this.chunks[end - 1] === 0x0d) end -= 1;
      if (end > this.maxFrame) {
        this.fail(new IpcFailure("frame_too_large", "incoming frame exceeds limit"));
        return;
      }
      const line = this.chunks.subarray(0, end).toString("utf8");
      this.chunks = this.chunks.subarray(nl + 1);
      let parsed: unknown;
      try {
        parsed = JSON.parse(line);
      } catch {
        this.fail(new IpcFailure("bad_frame", "incoming frame is not valid JSON"));
        return;
      }
      if (this.waiter) {
        const w = this.waiter;
        this.waiter = null;
        clearTimeout(w.timer);
        w.resolve(parsed);
      } else if (this.frames.length < 4) {
        this.frames.push(parsed);
      } else {
        this.fail(new IpcFailure("protocol", "unsolicited frames"));
        return;
      }
    }
  }

  private fail(f: IpcFailure): void {
    if (this.failure === null) this.failure = f;
    this.chunks = Buffer.alloc(0);
    if (this.waiter) {
      const w = this.waiter;
      this.waiter = null;
      clearTimeout(w.timer);
      w.reject(this.failure);
    }
    this.socket.destroy();
  }

  destroy(): void {
    this.fail(new IpcFailure("closed", "destroyed"));
  }

  write(frame: unknown): Promise<void> {
    const line = JSON.stringify(frame);
    if (Buffer.byteLength(line, "utf8") > this.maxFrame) return Promise.reject(new IpcFailure("frame_too_large_outgoing"));
    if (this.failure) return Promise.reject(this.failure);
    return new Promise<void>((resolve, reject) => {
      this.socket.write(`${line}\n`, (e) => (e ? reject(new IpcFailure("closed", e.message)) : resolve()));
    });
  }

  read(timeoutMs: number): Promise<unknown> {
    if (this.frames.length > 0) return Promise.resolve(this.frames.shift());
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiter = null;
        reject(new IpcFailure("timeout"));
      }, timeoutMs);
      this.waiter = { resolve, reject, timer };
    });
  }
}

function connect(endpoint: string, timeoutMs: number, maxFrame: number): Promise<Connection> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ path: endpoint });
    const timer = setTimeout(() => {
      socket.destroy();
      reject(new IpcFailure("timeout", "connect timeout"));
    }, timeoutMs);
    socket.once("connect", () => {
      clearTimeout(timer);
      // Do not keep the harness process alive because of an idle IPC connection.
      socket.unref();
      // Permanent listeners are attached synchronously so a late socket error is never unhandled.
      resolve(new Connection(socket, maxFrame));
    });
    socket.once("error", (e) => {
      clearTimeout(timer);
      socket.destroy();
      reject(new IpcFailure("unavailable", e.message));
    });
  });
}

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

export class KeplerClient {
  private readonly endpoint: string;
  private readonly clientName: string;
  private readonly clientVersion: string | undefined;
  private readonly connectTimeoutMs: number;
  private readonly handshakeTimeoutMs: number;
  private readonly requestTimeoutMs: number;
  private readonly maxFrameBytes: number;
  private conn: Connection | null = null;
  private inFlight = false;
  private seq = 0;

  constructor(o: ClientOptions) {
    this.endpoint = o.endpoint;
    this.clientName = o.clientName ?? "pi-adapter";
    this.clientVersion = o.clientVersion;
    this.connectTimeoutMs = o.connectTimeoutMs ?? 250;
    this.handshakeTimeoutMs = o.handshakeTimeoutMs ?? 500;
    this.requestTimeoutMs = o.requestTimeoutMs ?? 750;
    this.maxFrameBytes = o.maxFrameBytes ?? MAX_FRAME_BYTES;
  }

  /** Send one event and wait for the decision. Never throws. One request in flight; extra calls return `busy`. */
  async sendEvent(event: unknown): Promise<SendResult> {
    if (this.inFlight) return { ok: false, reason: "busy" };
    this.inFlight = true;
    try {
      const conn = await this.ensureConnection();
      const requestId = `r${++this.seq}`;
      await conn.write({ type: "event", request_id: requestId, event });
      const reply = await conn.read(this.requestTimeoutMs);
      if (!isRecord(reply)) throw new IpcFailure("protocol", "reply is not an object");
      if (reply["type"] === "error") throw new IpcFailure("server_error", String(reply["code"] ?? "unknown"));
      if (reply["type"] !== "decision" || reply["request_id"] !== requestId) throw new IpcFailure("protocol", "unexpected reply");
      const decision = reply["decision"];
      if (!isRecord(decision) || typeof decision["action"] !== "string") throw new IpcFailure("protocol", "malformed decision");
      return { ok: true, decision: decision as Decision, eventId: String(reply["event_id"] ?? "") };
    } catch (e) {
      // Any failure invalidates the connection; the next attempt reconnects.
      this.reset();
      return e instanceof IpcFailure ? { ok: false, reason: e.reason, detail: e.message } : { ok: false, reason: "closed" };
    } finally {
      this.inFlight = false;
    }
  }

  /** Drop the connection (idempotent). */
  close(): void {
    this.reset();
  }

  private reset(): void {
    this.conn?.destroy();
    this.conn = null;
  }

  private async ensureConnection(): Promise<Connection> {
    if (this.conn?.alive) return this.conn;
    this.reset();
    const conn = await connect(this.endpoint, this.connectTimeoutMs, this.maxFrameBytes);
    try {
      const hello: Record<string, unknown> = { type: "hello", protocol: PROTOCOL_VERSION, client: this.clientName };
      if (this.clientVersion !== undefined) hello["client_version"] = this.clientVersion;
      await conn.write(hello);
      const ack = await conn.read(this.handshakeTimeoutMs);
      if (!isRecord(ack)) throw new IpcFailure("protocol", "handshake reply is not an object");
      if (ack["type"] === "error") throw new IpcFailure("server_error", String(ack["code"] ?? "unknown"));
      if (ack["type"] !== "hello_ack" || ack["protocol"] !== PROTOCOL_VERSION) throw new IpcFailure("protocol", "unsupported handshake reply");
    } catch (e) {
      conn.destroy();
      throw e;
    }
    this.conn = conn;
    return conn;
  }
}
