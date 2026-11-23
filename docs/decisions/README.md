# Architecture Decision Records

Status of all records below: **proposed for implementation**. These capture the decisions made during design and may be superseded by later ADRs.

- [ADR-0001: Rust core](0001-rust-core.md)
- [ADR-0002: Capability-based adapters](0002-adapter-capabilities.md)
- [ADR-0003: Local storage](0003-local-storage.md)
- [ADR-0004: Conditional automation](0004-conditional-automation.md)
- [ADR-0005: Focus execution control](0005-focus-execution-control.md)
- [ADR-0006: Genesis design worker](0006-genesis-design-workflow.md)
- [ADR-0007: IPC wire protocol](0007-ipc-wire-protocol.md) (implemented in `kepler-ipc`)
- [ADR-0008: Delivery 0 runtime dependencies (SQLite binding, Tokio, serde, clap)](0008-storage-dependencies.md)

New ADR format: Context, Decision, Consequences, Alternatives, Status.
- [ADR-0009: Module pipeline, derived-context labels and Forge](0009-module-pipeline-and-forge.md) (implemented in `kepler-policy`, `kepler-forge`)
- [ADR-0010: Memory storage v2, ranking, consolidation and controls](0010-memory-storage-and-ranking.md) (implemented in `kepler-storage`, `kepler-memory`)
- [ADR-0011: Guardian and additive wire values](0011-guardian-and-wire-additions.md) (implemented in `kepler-policy`, `kepler-core`)
- [ADR-0012: MCP stdio server implemented in-tree](0012-mcp-stdio-minimal-server.md) (implemented in `kepler-mcp`)
- [ADR-0013: Pi automatic hooks](0013-pi-automatic-hooks.md) (implemented in `adapters/pi`)
- [ADR-0014: Focus implementation](0014-focus-implementation.md) (implemented in `kepler-focus`)
- [ADR-0015: Sentinel and audit findings](0015-sentinel-and-findings.md) (implemented in `kepler-sentinel`, `kepler-git`)
- [ADR-0016: Vision worker](0016-vision-worker.md) (implemented in `kepler-vision`, `workers/vision`)
- [ADR-0017: Orbit DAG scheduler](0017-orbit-dag-scheduler.md) (implemented in `kepler-orbit`)
- [ADR-0018: Genesis stays design-only](0018-genesis-deferred-design.md) (not implemented)
- [ADR-0019: rusqlite 0.41 and MSRV 1.89](0019-rusqlite-upgrade-and-msrv.md) (supersedes the version pin of ADR-0008)
- [ADR-0020: Guardian analyzes shell commands](0020-guardian-shell-analysis.md) (implemented in `kepler-policy::shell`)
- [ADR-0021: Memory captures observed facts and selects branch scope](0021-memory-auto-capture-and-branch.md) (implemented in `kepler-memory`, `kepler-events`)
- [ADR-0022: Sentinel runs the project's own checks](0022-sentinel-project-checks.md) (implemented in `kepler-sentinel`)
- [ADR-0023: Focus calibration, advisory display in Pi](0023-focus-calibration-and-advisory-display.md) (implemented in `kepler-focus`, `adapters/pi`)
- [ADR-0024: Genesis local UI](0024-genesis-local-ui.md) (implemented in `kepler-ui`)
- [ADR-0025: Claude Code hooks adapter (`kepler hook claude-code`)](0025-claude-code-hooks-adapter.md) (implemented in `kepler-cli`; tested with Claude Code 2.1.330 on Windows only)
