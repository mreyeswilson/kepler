# ADR-0019: rusqlite 0.41 (bundled SQLite 3.54) and minimum Rust 1.89

**Status:** Accepted; supersedes the version pin in ADR-0008 (same crate, same `bundled` feature)

## Context
`cargo deny check` (first run of a real cargo-deny 0.22.0 against this repository) reported RUSTSEC-2026-0459 /
CVE-2026-119375: bundled SQLite before 3.54.0 can return uninitialized page-cache memory when a database is opened
together with a `-wal` file of a different page size. Kepler bundles SQLite through `rusqlite` 0.32 (`libsqlite3-sys`
0.30, SQLite 3.46-era), so it was affected. The attack needs a less-trusted party to plant the database/WAL files in the
user's own data directory, which the per-user directory permissions already limit, but the advisory gate was red and the
fix is a version bump.

## Decision
* Upgrade `rusqlite` to 0.41 (`libsqlite3-sys` 0.39+, SQLite 3.54.x). No source change in Kepler was needed.
* Raise the workspace `rust-version` from 1.82 to 1.89, the MSRV that `rusqlite` 0.41 declares. CI and the repository
  toolchain use `stable`, so nothing else changes.
* Allow the `Zlib` license in `deny.toml` (permissive, OSI approved): `foldhash`, pulled in through `hashbrown` by
  `rusqlite`'s `hashlink`, uses it.
* New transitive crates from this bump: `foldhash`, `rsqlite-vfs`, `sqlite-wasm-rs`, `thiserror` 2 (and removed `ahash`,
  `zerocopy`, `once_cell`, `version_check`). `sqlite-wasm-rs`/`rsqlite-vfs` are `rusqlite`'s optional-target helpers; this
  was verified only by `cargo build`/`cargo deny`, not by reading their sources.

## Consequences
The advisory gate is green (`cargo deny check`: advisories, bans, licenses, sources ok). Building now needs a toolchain
>= 1.89. Existing databases are unaffected (same file format, schema versions unchanged).

## Alternatives
Ignoring the advisory in `deny.toml` (hides a real finding), linking a system SQLite (loses FTS5 and parity), staying on
0.32 until a later delivery (leaves the gate red).

## Measured side effect
In the in-process micro-benchmark the Memory FTS5 retrieval path went from about 350-550 us to about 820-950 us p50 (same
machine, benchmark rebuilt with 0.32 and 0.41 and the same Forge/Memory/Guardian pipeline). It stays far below the 100 ms
target and no cause was isolated. See `docs/development/PERFORMANCE.md`.
