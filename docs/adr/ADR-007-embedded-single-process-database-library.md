# ADR-007: Embedded Single-Process Database Library

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is designed as an embedded Rust library that runs in-process with the consumer application. The deployment unit is a single library crate published to crates.io.

## Decision
PulseDB is an embedded, single-process database library. The deployment unit is the `pulsehive-db` crate on crates.io. Multiple processes may open the same database file via file locking, but the library itself has no separate server process or worker threads. This design keeps the architecture simple and makes PulseDB a drop-in substrate for any Rust application.

## Touch surface
`src/lib.rs`, `src/db.rs`, `src/collective/`, `src/experience/`

## Revisit trigger
Not applicable — this is the core deployment model of PulseDB.
