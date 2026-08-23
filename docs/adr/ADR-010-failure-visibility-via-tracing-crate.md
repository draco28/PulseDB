# ADR-010: Failure Visibility via Tracing Crate

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB needs to make failures visible to operators for debugging and monitoring.

## Decision
PulseDB uses the `tracing` crate for structured logging. Failures are emitted as events with error levels. Critical operations (database open, write operations, errors) are instrumented with `#[instrument]`. Consumers configure RUST_LOG for verbosity. No metrics backend is included — consumers integrate with their own observability stack.

## Touch surface
`src/lib.rs`, `src/db.rs`, `CLAUDE.md`

## Revisit trigger
Revisit when observability requirements change (e.g., structured metrics export).
