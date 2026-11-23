# Threat model (initial)

## Assets
Source code, secrets, user prompts, design decisions, local memory database, session identifiers, tool invocations and any model provider credentials.

## Trust boundaries
1. User and harness -> native adapter
2. Adapter -> local IPC service
3. Context files, tool output and web data -> prompt/memory modules
4. Core -> external workers, agents and MCP clients
5. Core -> durable storage

## Threats and mitigations
| Threat | Planned mitigation |
|---|---|
| Prompt injection in retrieved memory | Trust labels, no privilege elevation, provenance and bounded contexts |
| Credential capture in memory/logs | denylist/sensitive-path filters, redaction, no raw logging by default |
| Untrusted plugin accessing core | user-scoped IPC auth, restricted socket/pipe permissions |
| Destructive commands | explicit approvals and harness permissions; fail closed |
| Data leakage to external models | no automatic off-device transmission; explicit provider configuration |
| Resource exhaustion | bounded queues, timeouts, cancel, worker process limits |
| Memory poisoning/stale facts | provenance, supersession, deletion and review |
| Malicious update/artifact | checksums/signatures and verified distribution (future design) |
| Cross-project data contamination | stable identity and scoped retrieval |

## Non-assumptions
A hook cannot govern operations outside its interception path. MCP alone cannot transparently intercept all agent prompts. SQLite databases are **not encrypted by default**; encryption is a separate design decision.

## Required security tests
Untrusted repo instructions, leaking .env via tool result, IPC access from other user, write cancellation, malformed events, path traversal, forged session IDs, and high-volume event flooding.
