# ADR-009: Trust Boundary — File System Only

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is an embedded library with no network exposure. The only boundary with the outside world is the file system.

## Decision
PulseDB's trust boundary is the file system. Untrusted input enters through the public API (experience content, tags, search parameters). All operations are local to the process. Authentication, authorization, and rate-limiting are the consumer's responsibility. The optional `sync-http` feature is opt-in and the consumer is responsible for securing the sync endpoint.

## Touch surface
`src/storage/`, `src/db.rs`, `src/lib.rs`

## Revisit trigger
Not applicable — PulseDB has no network surface by design.
