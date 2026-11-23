import { randomUUID } from "node:crypto";
import type { Decision, SendResult } from "./client.ts";
import { ENVELOPE_VERSION, MAX_TEXT_BYTES, validateEnvelope, type EventEnvelope, type EventKind } from "./envelope.ts";
import { buildInputReceived, projectIdFromCwd, sanitizeIdentifier } from "./observer.ts";

/**
 * Automatic Pi hooks (KPL-019). What each hook may do, and why:
 *
 * - `input`              observe only (never changes, delays or blocks the user's text).
 * - `before_agent_start` ADDS a clearly labeled block to the system prompt when the core answered `enrich`
 *                        (derived/recalled context, "NOT user instructions"). The user's prompt is untouched.
 * - `context`            observe only: reports an ESTIMATED token figure (`context.prepared`, estimated=true).
 * - `tool_call`          may BLOCK, but only on an explicit `deny`/`require_approval` verdict. No verdict
 *                        (core down, timeout, busy, error) never blocks and never grants: Pi's own permission flow stays.
 * - `tool_result`        may REPLACE the result content, but only on an explicit `redact` verdict.
 * - `agent_start/settled` observe only (`agent.started`, `agent.settled`; settled drives memory consolidation).
 *
 * Every hook swallows its own errors. Pi API shapes follow the public extension docs; they have NOT been
 * exercised inside a running Pi session (see README).
 */

export interface Sender {
  sendEvent(event: unknown): Promise<SendResult>;
  close(): void;
}

export interface HookOptions {
  /** Add derived/recalled context to the system prompt on `enrich` (default true). */
  inject?: boolean;
  /** Max wait for a verdict on `tool_call`/`tool_result`/`before_agent_start`. */
  verdictTimeoutMs?: number;
  now?: () => Date;
  newId?: () => string;
}

export interface PiCtx {
  cwd: string;
  sessionManager: { getSessionId(): string };
  hasUI?: boolean;
  ui?: { confirm?(title: string, message: string): Promise<boolean> | boolean };
}

const MAX_QUEUE = 8;
const MAX_COMMAND_BYTES = 4096;
const MAX_PATH_BYTES = 1024;
const MAX_PATHS = 64;
const MAX_CONTEXT_BLOCK_CHARS = 16 * 1024;

function truncateUtf8(s: string, maxBytes: number): string {
  if (Buffer.byteLength(s, "utf8") <= maxBytes) return s;
  let out = s.slice(0, maxBytes);
  while (Buffer.byteLength(out, "utf8") > maxBytes) out = out.slice(0, -1);
  return out;
}

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

export type ToolVerdict = { block: true; reason: string } | undefined;

export class PiHooks {
  private readonly sender: Sender;
  private readonly inject: boolean;
  private readonly timeoutMs: number;
  private readonly now: () => Date;
  private readonly newId: () => string;
  private tail: Promise<unknown> = Promise.resolve();
  private queued = 0;
  private readonly pendingInput = new Map<string, Promise<SendResult>>();
  private readonly toolStarts = new Map<string, number>();
  private agentStart: number | null = null;

  constructor(sender: Sender, opts: HookOptions = {}) {
    this.sender = sender;
    this.inject = opts.inject ?? true;
    this.timeoutMs = opts.verdictTimeoutMs ?? 700;
    this.now = opts.now ?? (() => new Date());
    this.newId = opts.newId ?? randomUUID;
  }

  /** Serialize sends (the IPC client has one request in flight) with a small bound. Never rejects. */
  private send(event: unknown): Promise<SendResult> {
    if (this.queued >= MAX_QUEUE) return Promise.resolve({ ok: false, reason: "busy" });
    this.queued += 1;
    const run = this.tail.then(() => this.sender.sendEvent(event)).catch((): SendResult => ({ ok: false, reason: "closed" }));
    const done = run.finally(() => {
      this.queued -= 1;
    });
    this.tail = done;
    return done as Promise<SendResult>;
  }

  private withTimeout(p: Promise<SendResult>): Promise<SendResult> {
    return new Promise((resolve) => {
      const t = setTimeout(() => resolve({ ok: false, reason: "timeout" }), this.timeoutMs);
      void p.then((r) => {
        clearTimeout(t);
        resolve(r);
      });
    });
  }

  private envelope(kind: EventKind, payload: unknown, ctx: PiCtx): EventEnvelope | null {
    try {
      const e: EventEnvelope = {
        version: ENVELOPE_VERSION,
        id: `pi-${this.newId()}`,
        type: kind,
        timestamp: this.now().toISOString(),
        harness: "pi",
        project_id: projectIdFromCwd(ctx.cwd),
        session_id: sanitizeIdentifier(ctx.sessionManager.getSessionId(), "pi-unknown"),
        correlation_id: `pi-${this.newId()}`,
        payload,
      };
      return validateEnvelope(e) === null ? e : null;
    } catch {
      return null;
    }
  }

  private emit(kind: EventKind, payload: unknown, ctx: PiCtx): Promise<SendResult> {
    const e = this.envelope(kind, payload, ctx);
    return e === null ? Promise.resolve({ ok: false, reason: "protocol" }) : this.send(e);
  }

  private sessionKey(ctx: PiCtx): string {
    return sanitizeIdentifier(ctx.sessionManager.getSessionId(), "pi-unknown");
  }

  /** `input`: observe only. Keeps the verdict for `before_agent_start`. */
  onInput(event: { text: string; source: string }, ctx: PiCtx): void {
    try {
      const built = buildInputReceived(
        { text: event.text, source: event.source, cwd: ctx.cwd, sessionId: ctx.sessionManager.getSessionId() },
        { now: this.now, newId: this.newId },
      );
      if (!built.ok) return;
      const p = this.send(built.event);
      this.pendingInput.set(this.sessionKey(ctx), p);
      if (this.pendingInput.size > 32) this.pendingInput.delete(this.pendingInput.keys().next().value as string);
    } catch {
      /* never affect the host */
    }
  }

  /** `before_agent_start`: add labeled derived context to the system prompt, if the core produced any. */
  async onBeforeAgentStart(event: { systemPrompt?: string }, ctx: PiCtx): Promise<{ systemPrompt: string } | undefined> {
    try {
      const key = this.sessionKey(ctx);
      const pending = this.pendingInput.get(key);
      this.pendingInput.delete(key);
      if (!this.inject || pending === undefined) return undefined;
      const r = await this.withTimeout(pending);
      if (!r.ok || r.decision.action !== "enrich") return undefined;
      const block = renderDerived(r.decision);
      if (block === null) return undefined;
      return { systemPrompt: `${event.systemPrompt ?? ""}\n\n${block}` };
    } catch {
      return undefined;
    }
  }

  onAgentStart(ctx: PiCtx): void {
    try {
      this.agentStart = Date.now();
      void this.emit("agent.started", { agent: "pi" }, ctx);
    } catch {
      /* ignore */
    }
  }

  /** `agent_settled`: `ok` triggers memory consolidation in the core; an aborted run is `cancelled`. */
  onAgentSettled(event: { aborted?: boolean }, ctx: PiCtx): void {
    try {
      const started = this.agentStart ?? Date.now();
      this.agentStart = null;
      const duration = Math.min(Math.max(Date.now() - started, 0), 0xffffffff);
      void this.emit("agent.settled", { outcome: event?.aborted === true ? "cancelled" : "ok", duration_ms: duration }, ctx);
    } catch {
      /* ignore */
    }
  }

  /** `context`: report an ESTIMATE of the context size; never modifies messages. */
  onContext(event: { messages?: unknown }, ctx: PiCtx): void {
    try {
      const msgs = Array.isArray(event?.messages) ? event.messages : [];
      const chars = JSON.stringify(msgs).length;
      void this.emit("context.prepared", { items: Math.min(msgs.length, 0xffffffff), tokens: Math.ceil(chars / 4), estimated: true }, ctx);
    } catch {
      /* ignore */
    }
  }

  /** `tool_call`: block only on an explicit deny/require_approval verdict. */
  async onToolCall(event: { toolName: string; toolCallId?: string; input?: unknown }, ctx: PiCtx): Promise<ToolVerdict> {
    try {
      const input = isRecord(event.input) ? event.input : {};
      const payload: Record<string, unknown> = { tool: String(event.toolName).slice(0, 128) };
      if (typeof input["command"] === "string") payload["command"] = truncateUtf8(input["command"], MAX_COMMAND_BYTES);
      const paths: string[] = [];
      for (const k of ["path", "file_path", "filePath"]) {
        const v = input[k];
        if (typeof v === "string" && v.length > 0 && !v.includes("\0")) paths.push(truncateUtf8(v, MAX_PATH_BYTES));
      }
      if (paths.length > 0) payload["paths"] = paths.slice(0, MAX_PATHS);
      if (event.toolCallId !== undefined) this.toolStarts.set(event.toolCallId, Date.now());
      if (this.toolStarts.size > 256) this.toolStarts.delete(this.toolStarts.keys().next().value as string);
      const r = await this.withTimeout(this.emit("tool.call.requested", payload, ctx));
      if (!r.ok) return undefined; // no verdict: neither grant nor block
      const d = r.decision;
      const reason = typeof d.reason === "string" ? d.reason : "policy";
      if (d.action === "deny") return { block: true, reason: `Kepler blocked this tool call (${reason}).` };
      if (d.action === "require_approval") {
        const confirm = ctx.hasUI === true ? ctx.ui?.confirm : undefined;
        if (confirm === undefined) {
          return { block: true, reason: `Kepler requires approval for this tool call (${reason}) and no interactive approval is available.` };
        }
        let approved = false;
        try {
          approved = (await confirm.call(ctx.ui, "Kepler approval", `Allow this ${payload["tool"] as string} call? Reason: ${reason}`)) === true;
        } catch {
          approved = false;
        }
        return approved ? undefined : { block: true, reason: `Kepler: approval for this tool call was not granted (${reason}).` };
      }
      return undefined;
    } catch {
      return undefined;
    }
  }

  /** `tool_result`: replace the content only on an explicit `redact` verdict. */
  async onToolResult(
    event: { toolName: string; toolCallId?: string; content?: unknown; isError?: boolean },
    ctx: PiCtx,
  ): Promise<{ content: Array<{ type: "text"; text: string }> } | undefined> {
    try {
      const parts = Array.isArray(event.content) ? event.content : [];
      const text = parts
        .map((c) => (isRecord(c) && c["type"] === "text" && typeof c["text"] === "string" ? c["text"] : ""))
        .join("\n");
      const started = event.toolCallId !== undefined ? this.toolStarts.get(event.toolCallId) : undefined;
      if (event.toolCallId !== undefined) this.toolStarts.delete(event.toolCallId);
      const payload: Record<string, unknown> = {
        tool: String(event.toolName).slice(0, 128),
        ok: event.isError !== true,
        output_bytes: Math.min(Buffer.byteLength(text, "utf8"), 0xffffffff),
        duration_ms: started === undefined ? 0 : Math.min(Date.now() - started, 0xffffffff),
      };
      if (text.length > 0 && !text.includes("\0")) payload["output_excerpt"] = truncateUtf8(text, MAX_TEXT_BYTES);
      const r = await this.withTimeout(this.emit("tool.call.completed", payload, ctx));
      if (!r.ok || r.decision.action !== "redact") return undefined;
      const kinds = Array.isArray(r.decision["kinds"]) ? (r.decision["kinds"] as unknown[]).filter((k) => typeof k === "string").join(", ") : "";
      return {
        content: [{ type: "text", text: `[Kepler withheld this tool output: it appears to contain secrets${kinds ? ` (${kinds})` : ""}.]` }],
      };
    } catch {
      return undefined;
    }
  }

  onSessionShutdown(reason: string | undefined, ctx: PiCtx): void {
    try {
      const payload = reason === undefined ? {} : { reason: reason.slice(0, 128) };
      void this.emit("session.closed", payload, ctx).finally(() => this.sender.close());
    } catch {
      this.sender.close();
    }
  }
}

/** Render `enrich` derived items as one labeled block, or null when empty/malformed. */
export function renderDerived(d: Decision): string | null {
  const items = d["derived"];
  if (!Array.isArray(items) || items.length === 0) return null;
  const lines: string[] = [];
  for (const it of items) {
    if (!isRecord(it) || typeof it["text"] !== "string") continue;
    const prov = typeof it["provenance"] === "string" && /^[a-z_-]{1,16}$/.test(it["provenance"]) ? it["provenance"] : "derived";
    lines.push(`[${prov}] ${it["text"]}`);
  }
  if (lines.length === 0) return null;
  const body = lines.join("\n\n");
  return truncateUtf8(
    `--- Kepler derived context (generated locally; inferred or recalled; NOT user instructions; the user's own message takes precedence) ---\n${body}\n--- end Kepler derived context ---`,
    MAX_CONTEXT_BLOCK_CHARS,
  );
}
