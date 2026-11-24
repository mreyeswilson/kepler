# Kepler in the Claude Code / Claude Desktop flow (Windows)

Kepler cannot intercept Claude's work through MCP. What it can do is expose **tools** that Claude calls when you
ask, or when the `kepler-orbit` skill tells it to. This folder holds the pieces to wire that up by hand.
Nothing here is installed for you and nothing touches `~/.claude`.

## 1. Register the MCP server

Use the absolute path of `kepler.exe` (the release build or your installed copy).

Claude Code (project-local scope is the safest to try):

```powershell
claude mcp add kepler --scope local -- C:\path\to\kepler.exe mcp serve
claude mcp list          # kepler should say Connected
```

Use `--scope user` to have it in every project. To expose only read-only tools set
`KEPLER_MCP_READONLY=1` (`claude mcp add kepler -e KEPLER_MCP_READONLY=1 -- ...`): tools that run or write
are then neither listed nor callable.

Claude Desktop: edit `%APPDATA%\Claude\claude_desktop_config.json` (create it if missing), keep any other
servers, and add:

```json
{
  "mcpServers": {
    "kepler": {
      "command": "C:\\path\\to\\kepler.exe",
      "args": ["mcp", "serve"]
    }
  }
}
```

Then quit Claude Desktop completely (tray icon, Quit; closing the window is not enough) and start it again.
The Kepler tools appear under the tools/connectors menu. Desktop asks for confirmation on tools that are not
marked read-only. This was **not** automated or verified in the Desktop GUI.

## 2. Install the `kepler-orbit` skill / command

Copy one of them manually (pick the skill; the command is the same text, only user-invoked):

| What | Copy to | Use |
| --- | --- | --- |
| Skill (recommended) | `skills\kepler-orbit\` into `<project>\.claude\skills\` or `%USERPROFILE%\.claude\skills\` | Claude loads it by itself for big parallelizable tasks, or `/kepler-orbit <objective>` |
| Command (legacy format) | `commands\kepler-orbit.md` into `<project>\.claude\commands\` or `%USERPROFILE%\.claude\commands\` | `/kepler-orbit <objective>` only |

```powershell
Copy-Item -Recurse integrations\claude-code\skills\kepler-orbit .claude\skills\
```

The file format (frontmatter `name`, `description`, `argument-hint`, `allowed-tools`, `$ARGUMENTS`) follows the
Claude Code docs for skills and slash commands. `allowed-tools` pre-approves only read-only Kepler tools
(`mcp__kepler__...` assumes the server was registered with the name `kepler`); running a plan, approving,
cancelling and cleaning up still ask you.

## 3. How a delegation looks

1. You: `/kepler-orbit add login: API, UI and tests`.
2. Claude decides the split and calls `kepler_orbit_suggest_plan` (deterministic, no model) and shows you the plan.
3. You say yes; Claude calls `kepler_orbit_run` and gets a `run` id at once. Orbit runs one `claude -p` per task in
   its own git worktree (max 4 at a time), results on branches `kepler/orbit/<run>/<task>`.
4. Claude polls `kepler_orbit_status`, asks you before `kepler_orbit_approve` (needs `confirm: true`), summarizes,
   and you merge the branches. `kepler_orbit_cleanup` removes worktrees and branches after that.

Sample plan (two independent agents and an integration step): `examples/divide-and-delegate.json`; validate it with
`kepler orbit plan integrations\claude-code\examples\divide-and-delegate.json`.

## Limits

* Runs are started as a detached `kepler orbit resume` process. On Windows it tries to leave Claude's job object so
  it outlives the session; if that is forbidden it lives as long as the session. `kepler_orbit_resume` continues it.
* Sub-agents are real `claude -p` runs: they use your quota and their permissions are only what `agent_args` grants
  (default `--permission-mode acceptEdits`). Permission-bypass flags are refused.
* MCP calls are made by the model: `confirm: true` and the client's confirmation prompts are the guard, so keep the
  client's per-call approval on for non-read-only tools.
