# ADR-0015: Sentinel deterministic audit and `audit_findings`

**Status:** Accepted, implemented in `kepler-sentinel`, `kepler-git`, `kepler-storage` (migration v3)

## Context
Delivery 3 asks for diff-aware risk rules and a findings schema. The v1 `files.changed` event carries only paths
(max 64): the daemon knows no repository root and must not read files or run project code. Findings must never
become a second place where secrets or prompts are stored.

## Decision
* Two entry points share one rule set. **Daemon:** `SentinelModule` applies *path rules* to `files.changed`
  (sensitive paths, CI workflows, dependency manifests, migrations, executable scripts, binary artifacts, `..`),
  persists hits through the bounded writer queue (`submit`, never blocks) and adds one `audit.high_severity_path`
  advisory (ADR-0014) when a high finding exists. **CLI:** `kepler sentinel audit` runs `git diff --unified=0`
  (+ at most 50 small untracked text files, never sensitive ones) through *line rules* on added text only.
* `kepler-git` runs git with a fixed argument vector (no shell), 15 s timeout with kill, 4 MiB stdout cap,
  `GIT_TERMINAL_PROMPT=0`, and a strict revision spelling check (no leading `-`, no `..`, no metacharacters).
* Rules are fixed substring/path/count checks, bounded to 20 findings per file, 500 per run, 500 files, 50 000 diff
  lines, 400 chars per line. No regex over untrusted input, no model, no network.
* `confidence` separates **confirmed** (specific secret formats such as tokens or private keys, sensitive paths,
  `..` paths) from **suspected** heuristics (unsafe code, CI changes, a generic `password = value` assignment,
  removed tests). Suspected findings are never presented as defects.
* `audit_findings` (migration v3): severity `info|low|medium|high|critical`, rule id, path, line, message
  (<=240 chars) and evidence (<=160 chars), both redacted before insert. Secret rules store only the kind
  (`secret kinds: github_token`), never the line. Deduplicated by (project, source/rule/path/evidence), so moving a
  line does not create a new row; capped at 5000 rows per project; findings can be dismissed (kept, hidden).
  The same table receives Vision findings (`source = vision`, ADR-0016).
* Commands: `kepler sentinel audit [--staged|--base REV] [--store] [--json] [--fail-on SEVERITY]`,
  `list`, `dismiss`. `audit` stores nothing unless `--store`.
* Disable the daemon module with `KEPLER_SENTINEL=off`.

## Consequences
Cheap and auditable, but shallow: no AST analysis, no data-flow, no integration with the project's own linters or
test runner (not implemented; that remains future work), English-centric substring rules, false positives and
negatives unmeasured. The daemon path rules do not inspect content. The generic credential pattern comes from the
shared redactor, so benign `password = ...` assignments may be reported as *suspected*.

## Alternatives
Reading files from the daemon (needs a trusted root, expands the attack surface); Tree-sitter (new dependency,
needs its own ADR); LLM review (cost, privacy).
