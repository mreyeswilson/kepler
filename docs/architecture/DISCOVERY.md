# Harness and desktop client discovery

**Status:** planned only, no executable detection in the current repository.

## Goal

`kepler detect` enumerates installed coding agents and desktop MCP clients, reports available integration mechanisms and versions, and never modifies user configuration. `kepler install <target>` or `kepler install --all` proposes changes and asks for approval before touching settings.

## Targets

| Client | Discovery targets | Integration strategy |
|---|---|---|
| Pi Coding Agent | command, local config, package directories | native TypeScript extension with lifecycle events |
| Claude Code | CLI and config | native hooks/plugin and MCP |
| Claude Desktop (chat) | app install and desktop extension/MCP config | MCP stdio or MCPB desktop extension |
| Claude Desktop Code tab | Claude Code-capable environment, app config | distinguish Code hooks from chat-only MCP capabilities |
| OpenAI Codex | CLI and settings | capability-specific plugin/hooks/MCP |
| Gemini CLI | CLI and extension config | native extension/hooks and MCP |
| OpenCode | CLI and plugin config | native plugin and MCP |
| Cursor | application and MCP config | MCP, optional native functionality only if documented |
| VS Code / GitHub Copilot | editor and MCP config | MCP; distinguish editor MCP and Copilot CLI settings |
| Windsurf | application and MCP config | MCP, depending on installed version |
| Generic MCP client | explicitly supplied config | MCP stdio/HTTP per client's requirements |

**Important:** support depends on installed versions, OS, and host policy. MCP exposes tools, but does not guarantee automatic preprocessing of every user message or capture of every agent event.

## Platform-aware discovery

- Linux: PATH, XDG directories and installed app metadata; honor Flatpak/Snap variants when supported.
- macOS: PATH, standard application directories and per-user Application Support locations.
- Windows: PATH, known app installs and user config paths; handle portable installs and named pipe support.
- WSL and remote development: report host vs guest explicitly; do not assume a Windows desktop app is installed within Linux.
- Avoid broad filesystem crawls and process inspection unless opted in.
- Do not read secrets or display unredacted MCP config environment values.

## Output contract

Each candidate is represented as:
```json
{
  "id": "claude-desktop",
  "installed": true,
  "version": null,
  "scope": "user",
  "environment": "macos-host",
  "integration": ["mcp-stdio", "mcpb"],
  "automation": ["tool-use-only"],
  "status": "available",
  "config_paths": ["<redacted-or-user-readable-path>"]
}
```

`installed` and `integration` must be grounded in explicit detection and known compatibility, not inferred merely from an app name. Distinguish unavailable, uncertain, permission-denied and unsupported.

## Safe setup

1. Detect candidates and version/capabilities.
2. Show planned additions and exact files to change; obtain approval.
3. Backup original config and use atomic writes.
4. Merge Kepler entry without deleting existing MCP servers or hooks.
5. Validate host config syntax and connection; rollback on failure.
6. Provide `kepler doctor` and uninstall/restore operations.

## Claude Desktop specifically

Claude Desktop chat can run local MCP servers and supports Desktop Extensions packaged as `.mcpb`; this is not equivalent to Claude Code's event hooks. Plan a `kepler mcp serve` integration and optionally a tested MCPB package. Automatic Forge-on-every-input and full lifecycle memory capture must **not** be advertised for chat-only MCP integration without a documented hook pathway.

Claude Code inside Desktop may expose additional hooks; detect and report the actual runtime rather than treating all of Claude Desktop as Claude Code.

## Acceptance criteria

- Read-only discovery of common clients with platform fixture tests.
- No false claim of hook-level automation for MCP-only clients.
- Existing user configurations unchanged by detection.
- Installation previews and explicit consent.
- Idempotent install, uninstall and rollback tests.
- No secrets in logs or diagnostics.
