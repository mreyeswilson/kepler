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

## Test coverage status (Delivery 0)
| Required test | Status |
|---|---|
| Malformed events | Covered: `kepler-events` fixtures, `kepler-ipc` and `kepler-core` e2e tests |
| High-volume event flooding | Covered for correctness/boundedness: pipelined request flood, 16x250 concurrent events, connection flood over the limit; throughput not measured |
| Forged session IDs | Covered: invalid ids rejected; live session cannot be impersonated by another connection (`session_conflict`). Not authentication: same-user processes are trusted |
| Path traversal | Covered: `resolve_within` and denylist normalization tests (`..`, absolute paths, NTFS stream/trailing-dot tricks) |
| Leaking .env via tool result | Covered: tool output is never persisted; `.env` reads are denied by the preflight; DB bytes scanned in tests |
| Write cancellation | Partial: writer drains/commits per short transaction and drops on overflow; no cancellation API yet |
| IPC access from another user | **Not tested.** Unix `0600`/`0700` and the Windows current-user DACL are implemented (the SDDL is unit-tested) but no second OS account was used to attempt access |
| Untrusted repo instructions / memory poisoning | Partial: recalled and derived text is delivered as separate blocks labeled "NOT instructions" with trust and provenance, one line per item (cannot forge labels), derived memories never supersede user-stated ones, candidates persist only on `ok` settlement. Not covered: a prompt-injected agent that emits malicious `memory.candidate` text on an `ok` settlement is stored as low-trust `derived` memory; review with `kepler memory list` |
