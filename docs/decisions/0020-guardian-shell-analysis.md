# ADR-0020: Guardian analyzes shell commands instead of matching phrases

Status: accepted

## Context
The preflight (ADR-0011) matched phrases inside the raw command string. Quoting
(`c""at .env`), variables (`a=.e; b=nv; cat $a$b`), wrappers (`sudo`, `bash -c`, `xargs`),
encoded PowerShell and `curl | sh` variants slip through or produce noise.

## Decision
`kepler-policy::shell` tokenizes the command (POSIX mode and, separately, a Windows mode for
PowerShell/cmd where backslash is literal and backtick/caret escape) and judges what would run:
statements, pipes, subshells, `$(..)`, backticks, process substitution, redirections,
heredocs into shells, quote concatenation, `$'..'`, brace expansion, tracked variable
assignments, wrapper peeling (`sudo env nohup timeout xargs find -exec bash -c eval cmd /c
powershell -Command/-EncodedCommand wsl`), git subcommand semantics (global options skipped),
cloud/CLI `delete|destroy|terminate`, SQL in client invocations, download-and-execute
(`curl|sh`, `iwr|iex`, `bash <(curl ..)`, base64 payload piped to a shell), environment dumps
and secret variables printed, writes to system paths/git internals/shell profiles.

* Sensitive paths found in any word or redirection -> `deny`. Globs that could match secret
  names (`.e*`, `id_*`, `*.pem`) -> `require_approval`.
* Existing paths are re-checked after `canonicalize` (symlinks, junctions, 8.3 names) against
  `KEPLER_PROJECT_ROOTS` (path list) or, if unset, the daemon's cwd; absolute paths always.
* Unresolvable program names (`$CMD`, `$(which rm)`) -> `require_approval`.
* Depth/node budgets; exceeding them -> `require_approval` (fail closed).
* The legacy phrase checks still run; the analyzer can only add restrictions.

## Consequences
Corpus `fixtures/guardian/bypass.txt` (deny / approve / pass lines, including false-positive
guards) is a regression suite. Remaining limits: scripts executed from files, aliases and shell
functions defined elsewhere, remote execution (`ssh host cmd`), and anything a harness does not
report are invisible. The analyzer is not a sandbox. Symlink resolution on Windows was verified
with a junction; file symlinks need Developer Mode and were not exercised on the author's machine.
