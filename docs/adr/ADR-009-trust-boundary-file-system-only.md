# ADR-009: Trust Boundary — File System Only

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is an embedded library with no network exposure. The only boundary with the outside world is the file system.

## Decision
PulseDB's trust boundary is the file system. Untrusted input enters through the public API (experience content, tags, search parameters). All operations are local to the process. Authentication, authorization, and rate-limiting are the consumer's responsibility. The optional `sync-http` feature is opt-in and, **when enabled, adds a second, network trust boundary**: remotely supplied `SyncChange` payloads are decoded and applied (`HttpSyncTransport`, `SyncServer`), so the filesystem-only model applies to default builds; sync-enabled deployments must threat-model the wire (the consumer hosts and secures the endpoint). A second network-supplied input exists with `builtin-embeddings`: model + tokenizer bytes are fetched from Hugging Face on first use (mutable `main`-branch URLs for the 768-d models) and then parsed — deployments that pin/cache model artifacts treat those bytes as filesystem input, everything else inherits the download path's integrity assumptions.

## Touch surface
`src/storage/`, `src/db.rs`, `src/lib.rs`, `src/sync/`, `src/embedding/`

## Revisit trigger
Revisit when the `sync-http` wire protocol or its threat model changes, when the model-download source or its URL pinning changes, or when a new network-facing feature is added.
