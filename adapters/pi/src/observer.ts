import { createHash, randomUUID } from "node:crypto";
import { KeplerClient, type FailureReason, type SendResult } from "./client.ts";
import { ENVELOPE_VERSION, MAX_TEXT_BYTES, validateEnvelope, type EventEnvelope, type InputReceivedPayload } from "./envelope.ts";

/** Where a Pi `input` event came from (mirrors Pi's `InputSource`; unknown values are treated as foreign). */
export type InputSource = "interactive" | "rpc" | "extension" | (string & {});

export interface InputObservation {
  /** The user's verbatim text. Never modified. */
  text: string;
  source: InputSource;
  cwd: string;
  sessionId: string;
  /** Current git branch when known. */
  branch?: string | undefined;
}

export interface BuildOptions {
  now?: () => Date;
  newId?: () => string;
}

const SAFE_ID = /[^A-Za-z0-9._:-]/g;

/** Make an arbitrary host string a valid Kepler identifier (`[A-Za-z0-9._:-]{1,128}`). */
export function sanitizeIdentifier(raw: string, fallback: string): string {
  const clean = raw.replace(SAFE_ID, "_").slice(0, 128);
  return clean.length > 0 ? clean : fallback;
}

/** Pseudonymous project id: never sends the filesystem path itself. */
export function projectIdFromCwd(cwd: string): string {
  return `pi-${createHash("sha256").update(cwd).digest("hex").slice(0, 16)}`;
}

export type SkipReason = "foreign_source" | "empty" | "oversized" | "contains_nul" | "invalid_envelope";

/** Map a Pi `input` event to an `input.received` v1 envelope, or explain why it is not forwarded. */
export function buildInputReceived(
  obs: InputObservation,
  opts: BuildOptions = {},
): { ok: true; event: EventEnvelope<InputReceivedPayload> } | { ok: false; skip: SkipReason } {
  if (obs.source !== "interactive" && obs.source !== "rpc") return { ok: false, skip: "foreign_source" };
  if (obs.text.length === 0) return { ok: false, skip: "empty" };
  // Text is never truncated or rewritten: if it cannot be forwarded verbatim, it is simply not observed.
  if (Buffer.byteLength(obs.text, "utf8") > MAX_TEXT_BYTES) return { ok: false, skip: "oversized" };
  if (obs.text.includes("\0")) return { ok: false, skip: "contains_nul" };
  const newId = opts.newId ?? randomUUID;
  const event: EventEnvelope<InputReceivedPayload> = {
    version: ENVELOPE_VERSION,
    id: `pi-${newId()}`,
    type: "input.received",
    timestamp: (opts.now ?? (() => new Date()))().toISOString(),
    harness: "pi",
    project_id: projectIdFromCwd(obs.cwd),
    session_id: sanitizeIdentifier(obs.sessionId, "pi-unknown"),
    correlation_id: `pi-${newId()}`,
    payload: obs.branch === undefined ? { text: obs.text } : { text: obs.text, branch: obs.branch },
  };
  if (validateEnvelope(event) !== null) return { ok: false, skip: "invalid_envelope" };
  return { ok: true, event };
}

export type Outcome =
  | { forwarded: true; decision: string }
  | { forwarded: false; why: SkipReason | FailureReason };

/**
 * Observe-only forwarder. The result is informational: this milestone never transforms,
 * blocks or delays the user's input, whatever the core answers (or fails to answer).
 */
export class InputObserver {
  private readonly client: Pick<KeplerClient, "sendEvent" | "close">;
  private readonly build: BuildOptions;

  constructor(client: Pick<KeplerClient, "sendEvent" | "close">, build: BuildOptions = {}) {
    this.client = client;
    this.build = build;
  }

  async observeInput(obs: InputObservation): Promise<Outcome> {
    try {
      const built = buildInputReceived(obs, this.build);
      if (!built.ok) return { forwarded: false, why: built.skip };
      const r: SendResult = await this.client.sendEvent(built.event);
      return r.ok ? { forwarded: true, decision: r.decision.action } : { forwarded: false, why: r.reason };
    } catch {
      return { forwarded: false, why: "closed" };
    }
  }

  close(): void {
    try {
      this.client.close();
    } catch {
      /* never fail the host */
    }
  }
}
