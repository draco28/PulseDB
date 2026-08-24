# ADR-009: Trust Boundary — File System Only

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is an embedded library with no network exposure. The only boundary with the outside world is the file system.

## Decision
PulseDB's trust boundary is the file system. Untrusted input enters through the public API (experience content, tags, search parameters). All operations are local to the process. Authentication, authorization, and rate-limiting are the consumer's responsibility. The optional `sync-http` feature is opt-in and, **when enabled, adds a second, network trust boundary**: remotely supplied `SyncChange` payloads are decoded and applied (`HttpSyncTransport`, `SyncServer`), so the filesystem-only model applies to default builds; sync-enabled deployments must threat-model the wire (the consumer hosts and secures the endpoint).

## Touch surface
`src/storage/`, `src/db.rs`, `src/lib.rs`, `src/sync/`

## Revisit trigger
Not applicable — PulseDB has no network surface by design.
