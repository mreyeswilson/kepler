/**
 * Vision worker protocol v1: one JSON object per line on stdin (requests) and stdout (responses).
 * stdout carries protocol frames only; diagnostics go to stderr. Mirrors `kepler-vision` (Rust).
 */

export const PROTOCOL_VERSION = 1;
export const MAX_REQUEST_BYTES = 64 * 1024;
export const MAX_RESPONSE_BYTES = 256 * 1024;
export const MAX_VIEWPORTS = 4;

export interface Viewport {
  name: string;
  width: number;
  height: number;
}

export type Request =
  | { v: 1; id: string; op: "hello"; launch?: boolean }
  | {
      v: 1;
      id: string;
      op: "check";
      url: string;
      viewports: Viewport[];
      timeout_ms: number;
      screenshot_dir?: string;
    }
  | { v: 1; id: string; op: "shutdown" };

export type ParseResult =
  | { ok: true; req: Request }
  | { ok: false; id: string; code: string; message: string };

export interface Issue {
  rule: string;
  count: number;
  samples: string[];
}

export interface ViewportResult {
  name: string;
  width: number;
  height: number;
  scroll_width: number;
  client_width: number;
  horizontal_overflow: boolean;
  console_errors: string[];
  page_errors: string[];
  failed_requests: string[];
  issues: Issue[];
  screenshot?: string;
}

export interface CheckResult {
  url: string;
  title: string;
  viewports: ViewportResult[];
}

export type Response =
  | { v: 1; id: string; ok: true; result: unknown }
  | { v: 1; id: string; ok: false; error: { code: string; message: string } };

const ID = /^[A-Za-z0-9._:-]{1,64}$/;
const NAME = /^[A-Za-z0-9_-]{1,32}$/;

function bad(id: string, code: string, message: string): ParseResult {
  return { ok: false, id, code, message };
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

export function parseRequest(line: string): ParseResult {
  if (Buffer.byteLength(line, "utf8") > MAX_REQUEST_BYTES) return bad("", "request_too_large", "request exceeds the size limit");
  let raw: unknown;
  try {
    raw = JSON.parse(line);
  } catch {
    return bad("", "malformed", "request is not valid JSON");
  }
  if (!isRecord(raw)) return bad("", "malformed", "request must be a JSON object");
  const id = typeof raw["id"] === "string" && ID.test(raw["id"]) ? raw["id"] : "";
  if (id === "") return bad("", "malformed", "missing or invalid id");
  if (raw["v"] !== PROTOCOL_VERSION) return bad(id, "unsupported_version", `protocol version ${String(raw["v"])} is not supported`);
  const op = raw["op"];
  if (op === "hello") return { ok: true, req: raw["launch"] === true ? { v: 1, id, op: "hello", launch: true } : { v: 1, id, op: "hello" } };
  if (op === "shutdown") return { ok: true, req: { v: 1, id, op: "shutdown" } };
  if (op !== "check") return bad(id, "unknown_op", "unknown op");

  const url = raw["url"];
  if (typeof url !== "string" || url.length === 0 || url.length > 2048) return bad(id, "invalid_request", "url must be a string up to 2048 characters");
  const vps = raw["viewports"];
  if (!Array.isArray(vps) || vps.length < 1 || vps.length > MAX_VIEWPORTS) {
    return bad(id, "invalid_request", `viewports must contain 1 to ${MAX_VIEWPORTS} entries`);
  }
  const viewports: Viewport[] = [];
  for (const vp of vps) {
    if (!isRecord(vp)) return bad(id, "invalid_request", "viewport must be an object");
    const { name, width, height } = vp as { name?: unknown; width?: unknown; height?: unknown };
    if (typeof name !== "string" || !NAME.test(name)) return bad(id, "invalid_request", "viewport name is invalid");
    if (!Number.isInteger(width) || (width as number) < 200 || (width as number) > 4000) return bad(id, "invalid_request", "viewport width must be 200..4000");
    if (!Number.isInteger(height) || (height as number) < 200 || (height as number) > 4000) return bad(id, "invalid_request", "viewport height must be 200..4000");
    viewports.push({ name, width: width as number, height: height as number });
  }
  const t = raw["timeout_ms"] ?? 15_000;
  if (!Number.isInteger(t) || (t as number) < 1000 || (t as number) > 120_000) return bad(id, "invalid_request", "timeout_ms must be 1000..120000");
  const dir = raw["screenshot_dir"];
  if (dir !== undefined && (typeof dir !== "string" || dir.length === 0 || dir.length > 512 || dir.includes("\0"))) {
    return bad(id, "invalid_request", "screenshot_dir is invalid");
  }
  const req: Request = { v: 1, id, op: "check", url, viewports, timeout_ms: t as number };
  if (typeof dir === "string") req.screenshot_dir = dir;
  return { ok: true, req };
}

export function okResponse(id: string, result: unknown): Response {
  return { v: 1, id, ok: true, result };
}

export function errResponse(id: string, code: string, message: string): Response {
  return { v: 1, id, ok: false, error: { code, message: message.slice(0, 300) } };
}

/** Serialize one frame; an oversized result is replaced by an error instead of flooding the pipe. */
export function encode(resp: Response): string {
  const s = JSON.stringify(resp);
  if (Buffer.byteLength(s, "utf8") > MAX_RESPONSE_BYTES) {
    return JSON.stringify(errResponse(resp.id, "result_too_large", "result exceeds the size limit"));
  }
  return s;
}
