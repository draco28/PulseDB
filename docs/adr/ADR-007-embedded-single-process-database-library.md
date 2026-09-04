# ADR-007: Embedded Single-Process Database Library

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is designed as an embedded Rust library that runs in-process with the consumer application. The deployment unit is a single library crate published to crates.io.

## Decision
PulseDB is an embedded, single-process database library. The deployment unit is the `pulsehive-db` crate on crates.io. Cross-process access is **serialized**: redb takes an exclusive file lock, so a second process opening the same path gets `DatabaseLocked` until the first closes (cross-process observation is likewise serialized in practice: WAL-sequence polling needs an opened storage handle, which the exclusive lock denies while another process holds the store — a poller process takes its turn between writer sessions). The library runs no daemon and no threads of its own (exception: with the `sync` feature, `SyncManager::start` launches a Tokio background loop that pushes on the configured interval — consumer-initiated, feature-gated); `hnsw_rs` parallel insert/rebuild borrows the shared Rayon pool (index build only), and with `builtin-embeddings` the ONNX Runtime session uses its default intra-op thread configuration (`create_session` does not pin `with_intra_threads`) — embedded consumers should budget for those dependency-managed threads. This design keeps the architecture simple and makes PulseDB a drop-in substrate for any Rust application.

## Touch surface
`src/lib.rs`, `src/db.rs`, `src/collective/`, `src/experience/`

## Revisit trigger
Not applicable — this is the core deployment model of PulseDB.
