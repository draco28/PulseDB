# ADR-007: Embedded Single-Process Database Library

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is designed as an embedded Rust library that runs in-process with the consumer application. The deployment unit is a single library crate published to crates.io.

## Decision
PulseDB is an embedded, single-process database library. The deployment unit is the `pulsehive-db` crate on crates.io. Cross-process access is **serialized**: redb takes an exclusive file lock, so a second process opening the same path gets `DatabaseLocked` until the first closes (concurrent cross-process observation goes through WAL-sequence polling, not a shared DB handle). The library runs no daemon and no threads of its own; `hnsw_rs` parallel insert/rebuild does borrow the shared Rayon pool (dependency-managed threads during index build only). This design keeps the architecture simple and makes PulseDB a drop-in substrate for any Rust application.

## Touch surface
`src/lib.rs`, `src/db.rs`, `src/collective/`, `src/experience/`

## Revisit trigger
Not applicable — this is the core deployment model of PulseDB.
