# Releasing Kepler

Nothing is released automatically. Two independent, manually triggered pipelines exist; **neither has ever been run**, and
the npm one is deliberately blocked until the gates below are changed by a maintainer.

| Pipeline | Workflow | Trigger | Result |
|---|---|---|---|
| Native binaries | `.github/workflows/release-binaries.yml` | tag `vX.Y.Z` (or manual dry run) | **Draft** GitHub Release with archives, `SHA256SUMS`, provenance attestations |
| Pi adapter on npm | `.github/workflows/release-npm.yml` | manual `workflow_dispatch` from tag `pi-vX.Y.Z`, plus environment approval | `npm publish --provenance` (blocked today) |

## Native binaries (`kepler` CLI/core)

Targets: `x86_64`/`aarch64` Linux (gnu), `x86_64`/`aarch64` macOS, `x86_64` Windows (msvc), all on native GitHub runners.

1. Make sure `main` is green (Rust CI, security).
2. Bump `[workspace.package] version` in `Cargo.toml`, update `Cargo.lock`, merge.
3. Optional dry run: Actions -> "Release binaries" -> Run workflow (builds and uploads workflow artifacts only; no release, no attestation).
4. Tag and push: `git tag vX.Y.Z && git push origin vX.Y.Z`. The tag must equal the `kepler-cli` version (the preflight job enforces it).
5. The workflow builds, smoke-tests (`kepler version`), packages, writes `SHA256SUMS`, attests provenance and creates a **draft** release.
6. Review the draft, check the assets, then publish it by hand in the GitHub UI.
7. Consumers verify with `sha256sum --check SHA256SUMS` and `gh attestation verify <archive> --repo <owner>/<repo>`.

Caveats: binaries are not code-signed or notarized (macOS Gatekeeper / Windows SmartScreen will warn); the license is still
`UNLICENSED`, so a public release should wait for the license decision.

## Pi adapter (`@kepler/pi`) on npm

AGENTS.md forbids publishing `adapters/pi` (and claiming it works) until Rust Core, hook integration, release tests, npm scope
ownership and license are approved. `release-npm.yml` enforces this: its `guard` job runs
`.github/scripts/check-npm-release-gates.mjs` and **fails** while any of these hold:

- `adapters/pi/package.json` has `"private": true`
- `license` is `UNLICENSED` or missing, or no `LICENSE` file exists
- `version` is `0.0.0`, no `test` script or no `test/*.test.*` files, install scripts or runtime dependencies present
- the run is not from tag `pi-v<version>`, `expected_version` differs, or the confirmation text is not exactly `publish <name>@<version>`

Manual steps pending before it can ever publish (each is a maintainer decision):

1. **Prove it works with real Pi**: an end-to-end test (currently only mock-core tests exist) and documented compatible Pi versions.
2. **Package name / npm scope**: reserve or confirm ownership of `@kepler` on npmjs.com, or pick another scope and update
   `package.json`, docs and workflows.
3. **License**: choose an SPDX license, add `LICENSE`, update `Cargo.toml` and `adapters/pi/package.json`.
4. **Trusted publisher**: on npmjs.com, package settings -> Trusted Publisher -> GitHub Actions, repository
   `<owner>/<repo>`, workflow `release-npm.yml`, environment `npm-release`. (A first publish of a brand-new package may need a
   one-time bootstrap by the owner; trusted publishing is configured per existing package.) No `NPM_TOKEN` is used or needed.
5. **GitHub environment**: create environment `npm-release` with required reviewers (and restrict it to tags `pi-v*`).
6. **Change the gates in a reviewed PR**: remove `"private": true`, set the license, set the real version.
7. **Tag**: `git tag pi-vX.Y.Z && git push origin pi-vX.Y.Z` (must equal `package.json` version).
8. **Run** Actions -> "Release npm (Pi adapter)" from that tag, with `expected_version` and
   `confirm` = `publish @scope/pi@X.Y.Z`; approve the `npm-release` environment prompt.
9. Verify the package page shows provenance, and install-test it (`pi install npm:<name>`).

Also pending for a real release: Core binary delivery and verification strategy for Pi users, changelog, and the acceptance
checklist in `docs/distribution/NPM_AND_PI.md`.

## Supply-chain hygiene

- Actions in the new workflows are pinned to commit SHAs (version in a trailing comment); Dependabot (`.github/dependabot.yml`) proposes updates.
- Default token permissions are `contents: read`; `contents: write`, `id-token: write` and `attestations: write` exist only in the release job, `id-token: write` only in the npm publish job.
- `security.yml` runs `cargo deny check` (see `deny.toml`) and `npm audit`.
- `ci.yml` still references floating action tags; pin it the same way in a follow-up.
