# ADR-014: Layered sync core with port traits and async edges

- **Status:** Accepted
- **Date:** 2026-08-23

## Context

PulseDB's guiding principle is "Storage, not Intelligence" with a "sync core, async edges" concurrency posture. The public surface (`PulseDB` struct, `SubstrateProvider`) must not leak runtime or storage coupling into consumers, and the storage/vector engines must stay swappable so ADR-001/ADR-005 decisions can be revisited without a public-API rewrite.

## Decision

The crate is a layered library: **public API** (`lib.rs`, `db.rs`) → **core services** (experience, search, relation, insight, activity, watch, substrate, sync) → **storage** (redb) and **vector index** (hnsw_rs), both behind internal port traits (`StorageBackend`, `VectorIndex`). The interior is synchronous; async exists only at the edges — the watch streams and the `#[async_trait] SubstrateProvider`. Dependency direction points inward: services never depend on a concrete engine, only on the ports.

## Consequences

- Engine swaps (a second storage backend, a different HNSW) are interior changes; the public API and `SubstrateProvider` contract hold.
- Adding a second consumer runtime (Python bindings, a server) extends an edge rather than piercing the core.
- Any new async in the interior is a boundary violation, not a convenience.

## Revisit trigger

Revisit when a second consumer runtime (bindings or server) forces a seam rethink.

### Verified claims

- Port traits and layering exist in the shipped crate (`src/storage`, `src/vector`, `src/substrate`); async confined to watch + substrate surfaces.

### Unverified claims

- None.
