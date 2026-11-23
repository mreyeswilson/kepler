# ADR-0001: Rust core

**Status:** Accepted for design

## Context
Kepler must run locally beside coding agents with minimal baseline overhead across Linux, macOS and Windows.

## Decision
Use Rust stable for core, CLI, event processing and storage; Tokio for async I/O. Use TypeScript only for harness integrations and Playwright where appropriate.

## Consequences
Strong control over allocations and native distribution; higher implementation complexity than Go. Measure actual RSS/CPU instead of asserting guaranteed efficiency.

## Alternatives
Go core (simpler MVP), Node-only service (easier integration, heavier resident runtime).
