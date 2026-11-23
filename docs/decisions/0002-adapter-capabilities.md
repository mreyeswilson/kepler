# ADR-0002: Capability-based adapters

**Status:** Accepted for design

## Context
Agent harnesses differ in hooks, tool semantics, lifecycle events and MCP support.

## Decision
Keep all harness-specific behavior in adapters. Normalize events and negotiate each capability explicitly; provide CLI/MCP fallback without claiming automatic interception.

## Consequences
More adapters to maintain, but core remains portable and capability gaps are visible.

## Alternatives
Rely only on MCP (cannot guarantee input interception), embed vendor APIs inside core (tight coupling).
