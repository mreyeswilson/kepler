# ADR-0003: Local storage

**Status:** Accepted for design

## Context
Memory and session state must be local, lightweight, durable and shared between installed harnesses.

## Decision
SQLite + WAL + FTS5, a single write coordinator and scoped memory with provenance. Embeddings optional; no cloud dependency.

## Consequences
Simple deployment and strong local control; must account for one active writer and protect sensitive data.

## Alternatives
PostgreSQL daemon, remote vector database, files-only storage.
