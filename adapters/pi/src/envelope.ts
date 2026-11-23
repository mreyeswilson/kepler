/**
 * Event envelope v1 (mirrors `kepler-events`). Only the pieces the Pi adapter needs:
 * a typed builder input and a validator used against the shared fixtures in /fixtures/events.
 * The Rust core remains the authority; this is a client-side pre-check so we never send frames
 * the core would reject.
 */

export const ENVELOPE_VERSION = 1;
export const MAX_ENVELOPE_BYTES = 65536;
export const MAX_TEXT_BYTES = 16 * 1024;
export const MAX_IDENTIFIER_BYTES = 128;
export const MAX_HARNESS_BYTES = 64;

export const EVENT_KINDS = [
  "input.received",
  "prompt.prepared",
  "context.requested",
  "context.prepared",
  "tool.call.requested",
  "tool.call.completed",
  "files.changed",
  "agent.started",
  "agent.settled",
  "memory.candidate",
  "session.closed",
] as const;
export type EventKind = (typeof EVENT_KINDS)[number];

export interface InputReceivedPayload {
  text: string;
}

export interface EventEnvelope<P = unknown> {
  version: typeof ENVELOPE_VERSION;
  id: string;
  type: EventKind;
  timestamp: string;
  harness: string;
  project_id: string;
  session_id: string;
  correlation_id: string;
  payload: P;
}

export type EnvelopeErrorCode = "malformed" | "unsupported_version" | "unknown_kind" | "invalid_field" | "field_too_large" | "oversized";

export interface EnvelopeError {
  code: EnvelopeErrorCode;
  message: string;
}

const ENVELOPE_KEYS = ["version", "id", "type", "timestamp", "harness", "project_id", "session_id", "correlation_id", "payload"];
const IDENT = /^[A-Za-z0-9._:-]+$/;
const RFC3339 = /^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+-]\d{2}:\d{2})$/;

const utf8Len = (s: string): number => Buffer.byteLength(s, "utf8");
const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const err = (code: EnvelopeErrorCode, message: string): EnvelopeError => ({ code, message });

function checkIdentifier(field: string, v: unknown, max: number): EnvelopeError | null {
  if (typeof v !== "string") return err("invalid_field", `${field} must be a string`);
  if (utf8Len(v) > max) return err("field_too_large", `${field} exceeds ${max} bytes`);
  if (!IDENT.test(v)) return err("invalid_field", `${field} must match [A-Za-z0-9._:-]+`);
  return null;
}

/** Payload shapes known to this adapter. Kinds not listed are only checked for being an object. */
function checkPayload(kind: EventKind, p: unknown): EnvelopeError | null {
  if (!isRecord(p)) return err("invalid_field", "payload must be an object");
  const only = (allowed: readonly string[]): EnvelopeError | null => {
    for (const k of Object.keys(p)) if (!allowed.includes(k)) return err("invalid_field", `payload.${k} is not allowed`);
    return null;
  };
  const text = (field: string, max: number, required: boolean): EnvelopeError | null => {
    const v = p[field];
    if (v === undefined) return required ? err("invalid_field", `payload.${field} is required`) : null;
    if (typeof v !== "string") return err("invalid_field", `payload.${field} must be a string`);
    if (utf8Len(v) > max) return err("field_too_large", `payload.${field} exceeds ${max} bytes`);
    if (v.includes("\0")) return err("invalid_field", `payload.${field} contains NUL`);
    return null;
  };
  switch (kind) {
    case "input.received":
      return only(["text"]) ?? text("text", MAX_TEXT_BYTES, true);
    case "session.closed":
      return only(["reason"]) ?? text("reason", 256, false);
    case "tool.call.requested": {
      const t = p["tool"];
      if (typeof t !== "string" || t.length === 0) return err("invalid_field", "payload.tool must be a non-empty string");
      return null;
    }
    default:
      return null;
  }
}

/** Validate an already-parsed JSON value. Returns `null` when the envelope is acceptable. */
export function validateEnvelope(value: unknown): EnvelopeError | null {
  if (!isRecord(value)) return err("malformed", "envelope must be a JSON object");
  if (value["version"] !== ENVELOPE_VERSION) return err("unsupported_version", "unsupported or missing version");
  for (const k of Object.keys(value)) if (!ENVELOPE_KEYS.includes(k)) return err("invalid_field", `unknown envelope field ${k}`);
  const kind = value["type"];
  if (typeof kind !== "string") return err("invalid_field", "type must be a string");
  if (!(EVENT_KINDS as readonly string[]).includes(kind)) return err("unknown_kind", `unknown event kind ${kind}`);
  for (const f of ["id", "project_id", "session_id", "correlation_id"] as const) {
    const e = checkIdentifier(f, value[f], MAX_IDENTIFIER_BYTES);
    if (e) return e;
  }
  const h = checkIdentifier("harness", value["harness"], MAX_HARNESS_BYTES);
  if (h) return h;
  const ts = value["timestamp"];
  if (typeof ts !== "string" || ts.length < 20 || ts.length > 40 || !RFC3339.test(ts) || Number.isNaN(Date.parse(ts.toUpperCase()))) {
    return err("invalid_field", "timestamp is not RFC 3339");
  }
  if (!("payload" in value)) return err("invalid_field", "payload is required");
  const pe = checkPayload(kind as EventKind, value["payload"]);
  if (pe) return pe;
  if (utf8Len(JSON.stringify(value)) > MAX_ENVELOPE_BYTES) return err("oversized", `envelope exceeds ${MAX_ENVELOPE_BYTES} bytes`);
  return null;
}

/** Validate raw text (as received from a fixture file or the wire). */
export function validateEnvelopeText(raw: string): EnvelopeError | null {
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return err("malformed", "not valid JSON");
  }
  return validateEnvelope(v);
}
