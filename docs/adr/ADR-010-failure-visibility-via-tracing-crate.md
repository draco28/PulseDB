# ADR-010: Failure Visibility via Tracing Crate

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB needs to make failures visible to operators for debugging and monitoring.

## Decision
PulseDB uses the `tracing` crate for structured logging. `#[instrument]` spans cover critical operations (open, writes, migration); **error-level events are not systematically emitted** — most `Err` returns travel to the caller without an `error!` event (only isolated sites log at error level today). Operators relying on error-level telemetry must wrap calls or configure `tracing` field-recording; systematic error events are a follow-up, not a shipped guarantee. Consumers configure RUST_LOG for verbosity. No metrics backend is included — consumers integrate with their own observability stack.

## Touch surface
`src/db.rs`, `src/storage/redb.rs`, `src/sync/manager.rs`

## Revisit trigger
Revisit when observability requirements change (e.g., structured metrics export).
