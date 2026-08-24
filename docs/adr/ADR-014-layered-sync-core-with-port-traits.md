# ADR-014: Layered sync core with port traits and async edges

- **Status:** Accepted
- **Date:** 2026-08-23

## Context

PulseDB's guiding principle is "Storage, not Intelligence" with a "sync core, async edges" concurrency posture. The public surface (`PulseDB` struct, `SubstrateProvider`) must not leak runtime or storage coupling into consumers, and the storage/vector engines must stay swappable so ADR-001/ADR-005 decisions can be revisited without a public-API rewrite.

## Decision

The crate is a layered library: **public API** (`lib.rs`, `db.rs`) → **core services** (experience, search, relation, insight, activity, watch, substrate, sync) → **storage** (redb) and **vector index** (hnsw_rs), both behind internal port traits (`StorageEngine`; `VectorIndex` for the index). The interior is synchronous; async exists only at the edges — the watch streams, the `#[async_trait] SubstrateProvider`, and the (feature-gated, off-by-default) `sync` transport/manager, which is async end to end and counts as an edge when enabled. **Runtime coupling:** those async edges call `tokio::task::spawn_blocking` / `tokio::spawn` directly (`src/substrate/impl.rs`, `src/sync/manager.rs`), so Tokio is the required edge runtime — awaiting the async surface from executor-agnostic code without a Tokio context can panic. Executor-neutral spawning is a revisit condition, not shipped. Dependency direction points inward: services never depend on a concrete **storage** engine, only on the port. (The vector index is *currently* wired concretely — `PulseDB` holds `HnswIndex` directly rather than `dyn VectorIndex`; the port exists but full dyn wiring is an aspiration, not shipped.).

## Consequences

- Engine swaps (a second storage backend, a different HNSW) are interior changes; the public API and `SubstrateProvider` contract hold.
- Adding a second consumer runtime (Python bindings, a server) extends an edge rather than piercing the core.
- Any new async in the interior is a boundary violation, not a convenience.

## Revisit trigger

Revisit when a second consumer runtime (bindings or server) forces a seam rethink.

### Verified claims

- `StorageEngine` port and layering exist in the shipped crate (`src/storage`, `src/substrate`).
- Known gap (honest record): vector access is concrete today (`HnswIndex` held directly in `src/db.rs`); the `sync` feature is an async edge; and the async edges require a Tokio runtime context — see Decision.

### Unverified claims

- None.
