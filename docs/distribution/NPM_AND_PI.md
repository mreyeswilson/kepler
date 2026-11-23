# Distribution: npm and Pi Coding Agent

**Status:** `adapters/pi` contains an experimental observe-only adapter (IPC client + `input` hook; mock-core tests only, **not verified with a real Pi session**). **No package is published**, the package stays `private: true`, and no native binaries are published yet. A guarded release workflow exists but cannot publish until the gates below are manually satisfied (see [RELEASING.md](../../RELEASING.md)).

## Why npm?
Pi can install a package via `pi install npm:<package>`. npm is a distribution channel for the Pi-specific TypeScript adapter, **not** Kepler's Rust core implementation.

## Planned artifacts
1. `kepler`: cross-platform Rust CLI/core, distributed as verified platform-specific binaries (release/install strategy to be decided).
2. `@kepler/pi` (provisional): npm Pi extension package with native hooks and IPC bridge.
3. Other harness adapters distributed using each harness's supported packaging.
4. Optional meta packages are **out of scope** for MVP.

The npm scope `@kepler` is not reserved or confirmed. If unavailable, choose a unique scope and update the docs, package manifest and workflows.

## Commands

Current developer-only scaffold, **not an operational Kepler install**:
```bash
git clone https://github.com/mreyeswilson/kepler.git
cd kepler
pi install ./adapters/pi
```

Intended future release:
```bash
pi install npm:@kepler/pi
```

Pi supports npm, Git and local package sources; declaring a `pi.extensions` path in package.json enables extension discovery.

A Git installation from the **repository root** will only be advertised after the root manifest/package layout has been tested for Pi; do not assume the nested package is automatically loaded.

## Installation lifecycle
1. Pi installs the adapter package.
2. Adapter checks whether Kepler Rust Core exists; it must not launch it or install anything without a safe documented mechanism.
3. If Core is missing, optional automation is disabled and Pi continues unchanged; give actionable diagnostics.
4. Once Core is installed, register appropriate input/context/tool/lifecycle hooks.
5. Apply user/project policies, budgets and permission checks.

## Publishing
- Initial package is marked `private: true` to prevent accidental `npm publish`.
- Package CI validates the manifest, runs typecheck/tests and inspects the tarball (`npm pack --dry-run`). It **never publishes**.
- After binary availability, integration tests, LICENSE selection and package scope verification, authorize the release.
- Prefer npm trusted publishing (OIDC) and provenance when npm account/repo configuration supports it; avoid long-lived automation tokens.
- Remove private flag and publish through a separately reviewed workflow with version/tag gate and explicit approval.
- Version the adapter and core compatibility contract independently. A package must reject unsupported protocol versions safely.
- `pi-package-check.yml` (CI) typechecks, builds, tests and validates the tarball contents (no publish).
- `release-npm.yml` is a manual, guarded workflow (environment `npm-release` with manual approval, trusted publishing via OIDC, `npm publish --provenance`). Its `guard` job **fails by design** while `private: true`, `UNLICENSED`, version `0.0.0`, a missing `LICENSE`/tests or a non-matching `pi-v<version>` tag are in place, so it cannot publish today. See [RELEASING.md](../../RELEASING.md) for the manual steps (npm scope ownership, license, trusted publisher, environment reviewers, flipping `private`, version, tag).
- Native binaries are released through `release-binaries.yml` (draft GitHub Release, `SHA256SUMS`, provenance attestations); it has not been run yet. The npm package never downloads or runs binaries.

## Release acceptance checklist
- [x] Rust Core/IPC exists and is tested (Delivery 0).
- [ ] Pi event hook behavior is version-tested.
- [x] Missing Core path passes through without blocking users (mock-core tests; not yet with real Pi).
- [ ] Linux/macOS/Windows binary delivery and verification are designed.
- [ ] Compatible package name/scope is owned.
- [ ] License file exists.
- [ ] Versioning/changelog/release process defined.
- [ ] CI passes and tarball contents verified.
- [ ] Explicit authorization to publish npm.
