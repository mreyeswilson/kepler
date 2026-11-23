import { resolveEndpoint } from "../src/endpoint.ts";
import { KeplerClient } from "../src/client.ts";
import { InputObserver } from "../src/observer.ts";

/**
 * Kepler Pi adapter (KPL-008, observe-only).
 *
 * Forwards each user `input` to the local Kepler core as an `input.received` v1 event and NOTHING else:
 * it never transforms, blocks, delays or drops the input (the handler returns immediately), and any
 * failure (core not running, timeout, bad frame) is silently a passthrough. It never launches the core.
 *
 * API NOTE: the Pi types below are minimal structural declarations, NOT imported from the Pi package.
 * They were compared by hand with `@earendil-works/pi-coding-agent` 1.6.0 (`on("input")`, `InputEvent`,
 * `ctx.cwd`, `ctx.sessionManager.getSessionId()`, `on("session_shutdown")`). This has NOT been exercised
 * inside a running Pi session: there is no end-to-end test with real Pi yet.
 */

interface PiInputEvent {
  type: "input";
  text: string;
  source: string;
}

interface PiContext {
  cwd: string;
  sessionManager: { getSessionId(): string };
}

interface PiExtensionApi {
  on(event: "input", handler: (event: PiInputEvent, ctx: PiContext) => void): unknown;
  on(event: "session_shutdown", handler: (event: unknown, ctx: PiContext) => void): unknown;
}

export const ADAPTER_VERSION = "0.0.0";

export default function keplerExtension(pi: PiExtensionApi): void {
  try {
    const client = new KeplerClient({ endpoint: resolveEndpoint(), clientVersion: ADAPTER_VERSION });
    const observer = new InputObserver(client);

    pi.on("input", (event, ctx) => {
      try {
        // Fire and forget: the input continues to Pi untouched and without waiting on Kepler.
        void observer
          .observeInput({
            text: event.text,
            source: event.source,
            cwd: ctx.cwd,
            sessionId: ctx.sessionManager.getSessionId(),
          })
          .catch(() => undefined);
      } catch {
        /* observation must never affect the host */
      }
      // Returning nothing means "continue" in Pi: the original text is passed through unchanged.
    });

    pi.on("session_shutdown", () => observer.close());
  } catch {
    /* registration failure: run without Kepler */
  }
}
