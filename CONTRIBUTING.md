# Contributing to Kepler

Kepler is documentation-first during its architectural phase. Issues and contributions should follow the [MVP roadmap](docs/roadmap/MVP.md) and [backlog](docs/roadmap/BACKLOG.md).

## Workflow
1. Open or select a scoped issue with clear acceptance criteria.
2. Review relevant ADRs and current harness documentation.
3. Propose architecture changes through a new ADR.
4. Use small branches and focused pull requests.
5. Add tests, measurements and docs when behavior changes.

## Engineering standards
- Rust stable, rustfmt and Clippy (once source code is introduced).
- Interfaces are typed and versioned; favor composition over highly generic abstractions.
- Benchmark cold start, peak/idle RAM and p95 latency for hot paths.
- Preserve user control and transparency. Interventions must be disableable.
- Avoid storing raw prompts or tool output by default.

## Security
Do not put secrets or personal code samples in issues or logs. Use redacted fixtures; report suspected vulnerabilities privately to repository maintainers.

## License
Licensing has not yet been selected; do not assume rights to reuse code outside GitHub's applicable terms until a license is published.
