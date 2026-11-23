import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

/**
 * Kepler Pi adapter scaffold.
 *
 * Deliberately registers NO hooks or tools until the Rust Core IPC protocol,
 * authentication, timeouts, and fallback behavior are implemented and tested.
 * Installing this package today does not enable Kepler Memory or Forge.
 */
export default function keplerExtension(_pi: ExtensionAPI): void {
  // Milestone KPL-008: implement guarded event forwarding to Kepler Core.
}
