// Test-only Pi extension (NOT part of the package): registers a scripted "faux" model so the whole
// Pi -> Kepler adapter -> core path can run without model credentials.
//
// The script comes from KEPLER_E2E_SCRIPT (JSON file): an array of steps
//   { "tool": "bash", "args": { "command": "..." } }  -> the model asks for a tool call
//   { "text": "..." }                                  -> the model answers with text
// Every request the "model" receives is appended to KEPLER_E2E_LOG (JSONL): the system prompt, the
// last user text and the last tool-result text. That is what the assertions inspect.
import { appendFileSync, readFileSync } from "node:fs";
import { fauxAssistantMessage, fauxProvider, fauxText, fauxToolCall } from "@earendil-works/pi-ai";

type Step = { tool?: string; args?: Record<string, unknown>; text?: string };

function textOf(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((c) => (c && typeof c === "object" && "text" in c ? String((c as { text: unknown }).text) : ""))
    .join("\n");
}

export default function (pi: { registerProvider(p: unknown): void }) {
  const scriptPath = process.env["KEPLER_E2E_SCRIPT"];
  const logPath = process.env["KEPLER_E2E_LOG"];
  if (!scriptPath || !logPath) return;
  const steps: Step[] = JSON.parse(readFileSync(scriptPath, "utf8"));
  let i = 0;
  const faux = fauxProvider({ models: [{ id: "scripted", name: "Scripted" }] });
  const respond = (context: { systemPrompt?: string; messages: Array<{ role: string; content?: unknown }> }) => {
    const msgs = context.messages ?? [];
    const lastUser = [...msgs].reverse().find((m) => m.role === "user");
    const lastTool = [...msgs].reverse().find((m) => m.role === "toolResult");
    appendFileSync(
      logPath,
      JSON.stringify({
        n: i,
        systemPrompt: [context.systemPrompt ?? "", ...msgs.filter((m) => m.role === "system").map((m) => textOf(m.content))].join(" | "),
        lastUser: lastUser ? textOf(lastUser.content) : "",
        lastToolResult: lastTool ? textOf(lastTool.content) : "",
      }) + "\n",
    );
    const step = steps[Math.min(i, steps.length - 1)] ?? { text: "done" };
    i += 1;
    if (step.tool) {
      return fauxAssistantMessage([fauxToolCall(step.tool, (step.args ?? {}) as never)], { stopReason: "toolUse" });
    }
    return fauxAssistantMessage([fauxText(step.text ?? "done")]);
  };
  faux.setResponses(Array.from({ length: 64 }, () => respond as never));
  pi.registerProvider(faux.provider);
}
