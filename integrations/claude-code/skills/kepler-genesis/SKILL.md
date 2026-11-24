---
name: kepler-genesis
description: Use when the user explicitly asks to design or redesign a user interface (a screen, flow or set of components) and wants options before code. Calls Kepler Genesis to get 1-3 UI specification candidates grounded in the project's own stack, design tokens and components, with HTML/PNG previews. Not for ordinary code edits, bug fixes or small styling tweaks.
argument-hint: [what to design]
allowed-tools: mcp__kepler__kepler_genesis_inspect mcp__kepler__kepler_genesis_list mcp__kepler__kepler_genesis_show
---

# Kepler Genesis: design first, then implement

Genesis produces **specifications and wireframe previews**, not final visuals and not code. You (the coding agent) implement the chosen candidate afterwards. The MCP tools below come from the `kepler` MCP server (`kepler mcp serve`); if they are not available, say so and stop, do not improvise a substitute.

Request: $ARGUMENTS

## When to use it

Only when the user asked to **design or redesign** an interface ("design a login screen", "redesign the dashboard", "give me options for the pricing page"). Do not run it for bug fixes, wiring, copy changes or a one-line style tweak. Never run it just because the project has a frontend.

## Steps

1. **Check what exists.** Call `kepler_genesis_inspect` (read-only, local, no model) with the project's absolute path. Note the stack, tokens and component library. If there are no tokens or components, tell the user designs will be framework-neutral.
2. **Tell the user what will leave the machine, then ask only if unclear.** `kepler_genesis_design` sends the brief (secrets redacted) plus the stack/token summary to Anthropic through the user's own `claude` CLI, takes roughly 30-120 s and writes files under the Kepler data directory. The user's request to design is the consent for one call; do not call it again for the same brief (an identical call returns the saved design). Use the user's own words as `brief`; do not add requirements they did not state.
3. **Generate.** Call `kepler_genesis_design` with `brief`, `project` (absolute path) and `candidates` (2 by default, max 3). On an error (model unavailable, timeout, invalid answer) report it plainly and offer to continue without Genesis; never retry in a loop.
4. **Present the candidates** briefly: for each, name, rationale, screens, planned files, and the paths of `preview_html` and `preview_png` (when present). Say that the previews are wireframes using the project's tokens, and mention `unknown_tokens` if any (the model referred to tokens the project does not define). Let the user open the files, then ask which candidate to implement, or what to change. Do not implement before they choose.
5. **Implement the chosen candidate.** Call `kepler_genesis_show` with `id` and `candidate` to get its full spec. Then:
   - Use the project's existing components (`library_component` entries) and design tokens; do not add colors, fonts or sizes that the spec or the project does not define.
   - Follow `implementation_plan` paths when they fit the repository; adjust to real folder conventions and say so.
   - Cover the empty/loading/error states, accessibility notes and responsive behavior in the spec.
   - Preserve existing business logic unless the user asked to change it. Keep the change within the screens in the spec.
6. **Verify** as you normally would (build, tests, and a browser check if Vision is set up). Report what was verified; do not claim the result matches the preview without checking.

## Manual installation

Genesis is opt-in and nothing installs this automatically.

1. Register the MCP server once: `claude mcp add kepler -- kepler mcp serve` (add `--scope user` for all projects).
2. Copy this folder to `.claude/skills/kepler-genesis/` in the project (shared with the repo) or to `~/.claude/skills/kepler-genesis/` (personal). On Windows PowerShell: `Copy-Item -Recurse integrations\claude-code\skills\kepler-genesis $HOME\.claude\skills\`.
3. Restart Claude Code. Invoke with `/kepler-genesis <what to design>`, or let Claude load it when you ask to design an interface.

`kepler_genesis_design` is deliberately not in `allowed-tools`: Claude Code will ask before running it, since it calls a model and writes files. Add it to your permission rules yourself if you want it to run without a prompt.

From a terminal the same flow is `kepler genesis inspect`, `kepler genesis design "<brief>"`, `kepler genesis list|show <id>|purge --yes`.
