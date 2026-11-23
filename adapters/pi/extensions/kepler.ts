import { resolveEndpoint } from "../src/endpoint.ts";
import { KeplerClient } from "../src/client.ts";
import { PiHooks, type PiCtx } from "../src/hooks.ts";

/**
 * Kepler Pi adapter (KPL-008 input observation, KPL-019 automatic hooks).
 *
 * Hooks and what they may do (see src/hooks.ts):
 *  - input: observe only; before_agent_start: appends a labeled derived-context block to the system prompt
 *    (never edits the user's prompt); context/agent_start/agent_settled/session_shutdown: observe only;
 *  - tool_call: blocks ONLY on an explicit deny/require_approval verdict (no verdict never blocks, never grants);
 *  - tool_result: replaces content ONLY on an explicit redact verdict.
 * Any failure (core not running, timeout, bad frame) means Pi proceeds exactly as without Kepler.
 * It never launches the core. `KEPLER_PI_INJECT=0` turns the system-prompt addition off.
 *
 * API NOTE: the Pi types below are minimal structural declarations, NOT imported from the Pi package, written
 * from Pi's published extension documentation (events before_agent_start, context, tool_call, tool_result,
 * agent_start, agent_end/agent_settled, session_shutdown). This has NOT been exercised inside a running Pi
 * session: there is no end-to-end test with real Pi yet.
 */

type Handler<E> = (event: E, ctx: PiCtx) => unknown;

interface PiExtensionApi {
  on(event: "input", h: Handler<{ type: "input"; text: string; source: string }>): unknown;
  on(event: "before_agent_start", h: Handler<{ prompt?: string; systemPrompt?: string }>): unknown;
  on(event: "context", h: Handler<{ messages?: unknown }>): unknown;
  on(event: "agent_start", h: Handler<unknown>): unknown;
  on(event: "agent_settled", h: Handler<{ aborted?: boolean }>): unknown;
  on(event: "tool_call", h: Handler<{ toolName: string; toolCallId?: string; input?: unknown }>): unknown;
  on(event: "tool_result", h: Handler<{ toolName: string; toolCallId?: string; content?: unknown; isError?: boolean; input?: unknown }>): unknown;
  on(event: "session_shutdown", h: Handler<{ reason?: string }>): unknown;
}

export const ADAPTER_VERSION = "0.0.0";

export default function keplerExtension(pi: PiExtensionApi): void {
  try {
    const client = new KeplerClient({ endpoint: resolveEndpoint(), clientVersion: ADAPTER_VERSION });
    const hooks = new PiHooks(client, { inject: process.env["KEPLER_PI_INJECT"] !== "0" });

    pi.on("input", (event, ctx) => {
      hooks.onInput(event, ctx);
      // undefined = continue: the original text is passed through unchanged.
    });
    pi.on("before_agent_start", (event, ctx) => hooks.onBeforeAgentStart(event, ctx));
    pi.on("context", (event, ctx) => {
      hooks.onContext(event, ctx);
    });
    pi.on("agent_start", (_event, ctx) => {
      hooks.onAgentStart(ctx);
    });
    pi.on("agent_settled", (event, ctx) => {
      hooks.onAgentSettled(event, ctx);
    });
    pi.on("tool_call", (event, ctx) => hooks.onToolCall(event, ctx));
    pi.on("tool_result", (event, ctx) => hooks.onToolResult(event, ctx));
    pi.on("session_shutdown", (event, ctx) => {
      hooks.onSessionShutdown(event?.reason, ctx);
    });
  } catch {
    /* registration failure: run without Kepler */
  }
}
