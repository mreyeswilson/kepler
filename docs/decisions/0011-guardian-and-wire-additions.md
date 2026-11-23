# ADR-0011: Guardian (preflight, approvals, result scan, context budget) and additive wire values

**Status:** Accepted (implemented in Delivery 2: `kepler-policy::guardian`, `kepler-core::pipeline`)

## Context
KPL-013 asks for fail-closed tool preflight, a denylist of destructive paths/commands, explicit approvals,
secret-leak detection in tool results, and token accounting that separates measured from estimated figures.
Delivery 0 already had a minimal preflight; the protocol had no way to say "this result contains secrets".

## Decision

### Preflight (fail-closed, always on)
`Guardian::preflight` = built-in checks, then optional user rules. Order: sensitive path (`.env`, keys...) -> `deny`;
user `deny-path`/`deny-command` -> `deny`; destructive command, harness `destructive` hint, path traversal, system
locations (`/etc`, `/usr`, `C:\Windows`...), git internals (`.git/hooks`, `.git/config`, `.gitmodules`) or user
`approve-*` rules -> `require_approval`; otherwise `passthrough`. A panic inside preflight yields `deny`.
The destructive phrase list gained publish/destroy/delete style commands (npm/cargo publish, terraform destroy,
kubectl delete, docker system prune/push, aws s3 rm, git push --delete, shutdown, diskpart...). Matching is string based
on a whitespace-collapsed lowercase command: it is a speed bump for honest mistakes, **not a sandbox** (an obfuscated or
unreported command is invisible to it).

### User rules (`<data dir>/guardian-rules.txt` or `KEPLER_GUARDIAN_RULES`)
Line format `deny-path|approve-path|deny-command|approve-command <pattern>`, max 256 rules of <= 200 chars. Rules can only
*add* restrictions; there is no allow rule and built-in denials always win. Bad lines are reported as daemon warnings and
ignored; an unreadable file leaves the built-ins in force. No TOML/regex dependency.

### Approvals
Guardian returns `require_approval`; it never approves, never stores approvals and never accepts an approval claimed in an
event (events are untrusted). The harness must ask the user. Every verdict is recorded as metadata (`events.decision`).

### Result scan
`tool.call.completed.output_excerpt` (in memory only, never persisted) is checked with the redactor's detectors.
Secrets found -> `{"action":"redact","reason":"secret_in_tool_result","kinds":["aws_access_key"]}`: coarse categories, never
the secret. The adapter decides how to withhold the result. If the scan itself fails the verdict is `redact`
(`scan_failure`), i.e. fail closed. Detection is the redactor's pattern set (key=value with sensitive names, PEM blocks,
JWTs, common token prefixes, high-entropy tokens); it will miss unknown formats and may flag harmless high-entropy strings.

### Budget stage
`BudgetStage` (the `guardian` pipeline stage, `KEPLER_GUARDIAN_BUDGET=off` disables) drops near-duplicate derived items
(word Jaccard >= 0.9) and keeps whole items, in pipeline order, until 1200 estimated tokens (or the `budget_tokens` of a
`context.requested`, if lower). Text is never truncated or rewritten.

### Token accounting
`estimate_tokens` (chars-per-token by model family; non-ASCII ~1.5 chars/token) is a heuristic, no tokenizer is bundled,
and every figure carries a `Basis` (`estimated`/`measured`). The enrich verdict carries `estimated_tokens`. `context_metrics`
rows store `estimated` as a flag: Kepler's derived context and memory recall are always estimates; a harness
`context.prepared` keeps the harness's own `estimated` value. The two are never summed.

### Additive wire values (protocol stays 1)
* `decision.action = "redact"` with `reason` and `kinds`.
* `estimated_tokens` (number) next to `derived` on `enrich` verdicts.
* `derived[].provenance = "recalled"` (see ADR-0009).
Adapters must treat unknown actions as passthrough *for input events* and as "no restriction beyond the harness's own
permissions" for tool events, exactly as before; a `redact` they do not understand simply leaves the result unchanged.

## Consequences
Guardian costs one pass over the tool command/paths or a <= 16 KiB excerpt; it adds no dependency. Because the
harness decides what to send, coverage equals the adapter's hook coverage.

## Alternatives
Regex-based rule engine (dependency/ReDoS), a persistent approval allowlist (risk of silent privilege growth), model-based
secret detection (privacy/cost), shipping a tokenizer per model family (size, licensing; revisit when measured error matters).
