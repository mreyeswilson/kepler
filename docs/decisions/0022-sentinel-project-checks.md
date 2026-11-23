# ADR-0022: Sentinel runs the project's own checks, on request

Status: accepted. Extends ADR-0015.

## Decision
`kepler sentinel check --repo <dir> [--cmd "name=program args"]... [--detect] [--timeout-secs N]
[--yes] [--store] [--json] [--fail-on sev]` runs lint/test commands inside the working tree and
records their results as `audit_findings`.

* Commands are argv lists (no shell). They come only from `--cmd` or from `--detect`, which maps
  manifests present in the tree to defaults (Cargo.toml -> `cargo clippy --all-targets`,
  `cargo test`; package.json `lint`/`typecheck`/`test` scripts -> `npm run <script>`; pytest;
  go.mod -> `go vet`, `go test`). A repository file can never make Kepler run something on its
  own: running `--detect` on an untrusted repo executes that repo's scripts, which is exactly what
  the user asked for and is documented as such.
* Each command goes through the Guardian preflight (same rules file): `deny` never runs,
  `require_approval` only with `--yes`.
* Bounded: per-command wall-clock timeout with process-tree kill, 256 KiB of output per stream,
  no stdin, `CI=1 NO_COLOR=1`, the repository directory is not searched for the program, at most
  200 findings.
* Output parsing: rustc/cargo (`error[..]` + `-->`), `path:line:col: error|warning`, tsc,
  pytest `FAILED`, jest/vitest `FAIL`, `cargo test ... FAILED`. A failing command with nothing
  parseable yields one `check.failed` finding with the exit status. Sensitive paths are dropped.
  Findings are `confirmed` because the tool reported them; `sentinel check` never claims to find
  bugs by itself.
* Not wired into the daemon: the daemon knows no repository root and must not run project code
  as a side effect of an event. A harness or the user invokes the command.

## Limits
Parsers are heuristics over text output (no SARIF/JSON integration yet); no AST analysis.
