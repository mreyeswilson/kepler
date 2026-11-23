# Kepler Pi package — scaffold

This directory is the future Pi Coding Agent distribution unit.

> **Not functional yet.** It is a loadable no-op extension skeleton. Installing it does **not** enable Memory, Forge or other Kepler features.

## Local developer installation

From the repository root:

```bash
pi install ./adapters/pi
```

Pi may resolve relative sources based on its settings location; use an **absolute path** if needed. To test without permanent installation, consult the current Pi extension/package documentation.

## Intended public installation (not available)

```bash
pi install npm:@kepler/pi
```

`@kepler/pi` is a **provisional package name**. The npm `@kepler` scope and publishing authorization are not verified. We must secure or change the scope before publishing.

## Layout

- `package.json`: npm package metadata and Pi's explicit extension discovery path.
- `extensions/kepler.ts`: no-op Pi extension scaffold.

## Release requirements

- Rust Core binary available for supported platform, with checksum/signature verification.
- A tested IPC event adapter and graceful passthrough when Kepler is missing.
- Automated package checks and integration tests on supported Pi versions.
- An explicit LICENSE chosen and added to the repository.
- Package scope ownership and npm trusted publishing configured.
- Remove `private: true` only after release criteria are satisfied.

**Packaging plan:** Publish an npm adapter package and distribute signed Rust native binaries separately as optional platform packages or releases. Do not download/execute remote binaries in npm install scripts without explicit architecture and security review.
