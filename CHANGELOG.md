# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Sync identity API (`sync` feature): `PulseDB::instance_id()` + `PulseDB::remint_instance_id()`.** `instance_id()` exposes the store's persistent `InstanceId`. `remint_instance_id()` gives a *restored file copy* of a store a fresh identity in one write transaction so the original and the copy stop sharing a per-instance reinforcement bucket — the exact-total guarantee of the G-counter merge (FR-031) holds only across distinct ids. Old buckets are left untouched (totals preserved); no WAL or sync event is emitted; the remint is a `tracing::info!` carrying the old and new ids; read-only stores return `PulseDBError::ReadOnly`. Call it **before** constructing a `SyncManager`. Explicit API only — no heuristic clone detection. `StorageEngine` gains the matching `remint_instance_id` port method. Closes #10.
- **Real-fixture sentinel-merge test.** `test_create_collision_sentinel_merge_does_not_double_count` now migrates two copies of the committed `real-v0.4.0.redb` (schema v2, scalar `applications`) through the genuine v2→v3 path and syncs them, proving the `{LEGACY}` sentinel bucket merges once rather than doubling. The fixture-copy helper moved to `tests/common/mod.rs`. Closes #11.
- **Sync request byte cap (#26).** `SyncConfig::max_request_bytes` (default 16 MiB, `validate()` rejects 0) bounds every body the sync server accepts: `SyncServer::handle_{handshake,push,pull}_bytes` compare `bytes.len()` **before** the wire-preamble read and before any postcard decode, refusing an oversized body with the new typed `SyncError::PayloadTooLarge { size, max }` (`is_payload_too_large()` for a `413` mapping). `HttpSyncTransport` applies the same cap to response bodies — a `Content-Length` above the cap is refused unread, a chunked body is read bounded — tunable via `with_max_response_bytes()`. `sync` / `sync-http` features only.
- **Typed protocol-version mismatch on the sync client (#12).** `SyncManager` now checks the peer's `protocol_version` before mapping a soft `accepted: false` handshake, so a version-mismatched server reaches callers as `SyncError::ProtocolVersion { local, remote }` instead of a reason string inside `SyncError::Handshake`. The handshake capability list remains informational (advertised, never negotiated) — documented in the `sync` module.
- **Clock-skew visibility for reinforcement timestamps (#13).** `SyncConfig::max_clock_skew_ms` (default 300 000) and a local-only `SyncStats { skewed_timestamps }` exposed through `SyncManager::stats()` and `SyncServer::stats()`. An incoming `last_reinforced` beyond `now + max_clock_skew_ms` is logged at `warn` (peer, experience id, skew) and counted once per incoming change, before it is applied; the value is merged **unchanged** — FR-031's max-merge is never clamped, rejected or re-timestamped, so convergence is untouched. The bound is advisory until protocol v5 carries a record-level time reference. `SyncStats` is not a wire type; `SYNC_PROTOCOL_VERSION` stays at 4.

### Changed
- **MSRV 1.89 → 1.90.** Required by redb 4.2 (`rust-version = "1.90"`); the CI job is now named `MSRV` (version-agnostic). Recorded in ADR-008.
- **A push is acknowledged only up to the last change that applied.** `SyncServer::handle_push` previously returned the highest sequence in the batch regardless of per-change failures, and the sender persisted that as its `push_sequence` — so `compact_wal` could delete a local WAL event the peer had rejected. The applier now reports `ApplyResult::safe_through`, the highest sequence at or below which every change was applied, resolved, or idempotently skipped, and the server acknowledges that instead. A batch whose first change fails acknowledges nothing, which leaves the peer at `push_sequence == 0` and blocks compaction.
- **An empty pull still registers the peer.** The pull position is now persisted on every successful pull, not only when the batch had changes, so a `PullOnly` peer that has never received anything still appears in the cursor store and holds compaction at `push_sequence == 0`.
- **Sync cursors split into push/pull positions (schema v5, 0.8.0 minor per ADR-008).** `SyncCursor` is now `{ instance_id, push_sequence, pull_sequence }` — the local WAL sequence a peer has acknowledged and the remote WAL sequence applied from it — instead of one `last_sequence` slot the push and pull paths both overwrote. `PulseDB::compact_wal` now trusts `min(push_sequence)` only; pull positions never feed compaction, so a remote pull position can no longer delete unpushed local events. A peer at `push_sequence == 0` (never pushed to, or `SyncDirection::PullOnly`) blocks compaction until a push happens. `StorageEngine` gains `update_push_cursor` / `update_pull_cursor` (single-transaction read-modify-write per side). Closes #9.
- **New wire type `SyncPosition { instance_id, sequence }`** (`pulsedb::sync::types::SyncPosition`) carries the single-direction position in `PullRequest.cursor`, `PullResponse.new_cursor` and `PushResponse.new_cursor`. Its postcard bytes are identical to the 0.7.0 wire `SyncCursor`, so **`SYNC_PROTOCOL_VERSION` stays 4** and 0.7.x peers interoperate; a wire cursor carrying both positions is a protocol v5 change.

### Added
- **Real v0.7.0 golden fixture** (`tests/fixtures/real-v0.7.0.redb` + manifest, generated by the out-of-workspace `tools/fixture-gen-v0_7_0` against the published `pulsehive-db =0.7.0` with `sync`): a schema-4 store carrying a genuine `last_sequence > 0` cursor. `tests/storage_format_upgrade.rs` proves the v5 migration on it with and without the `sync` feature: `.pre-v5.bak` byte-identical to the fixture, cursor reset to 0/0, entities value-identical, `schema_version == 5`.
- **Pristine schema-migration sidecars.** For a store that is already redb-v3 (every 0.6.x/0.7.x file), the `.pre-v4.bak` / `.pre-v5.bak` copy is now claimed **before the first writable redb open** (a read-only open peeks at `schema_version`), so it is byte-identical to the file the operator had on disk — redb 4.x rewrites a store's allocator pages on every writable open+close, which made the post-open copy a valid but not byte-identical backup. Falls back to the post-open copy when the peek cannot run (crashed session, locked file); redb-v2 stores are unaffected — their pristine copy is the existing `.pre-substrate.bak`.

### Breaking
- **Schema v4→v5 on-disk migration.** Existing stores migrate automatically on the first **writable** open: `<db>.pre-v5.bak` is claimed first, then every sync cursor is rewritten to `{ push_sequence: 0, pull_sequence: 0 }` (the legacy `last_sequence` is discarded — never used to seed either side — and logged at `warn` per peer). A read-only open of a not-yet-migrated store returns a typed `ReadOnly` error instead of migrating. See [docs/storage-migration.md](docs/storage-migration.md#schema-v5-080-sync-cursors-reset).
  - **Migration note:** expect **one full idempotent resync per peer** after the upgrade, and WAL compaction stays blocked until the next push to each peer. **Events compacted before the upgrade are not recoverable** — if a pre-0.8.0 compaction already deleted unpushed events (the #9 failure), the resync covers only what is still in the WAL. Rollback: reinstall 0.7.x and restore `.pre-v5.bak`.
- **`SyncCursor.last_sequence` removed** (replaced by `push_sequence` / `pull_sequence`); `PullRequest.cursor`, `PullResponse.new_cursor` and `PushResponse.new_cursor` are `SyncPosition` (was `SyncCursor`). Custom `SyncTransport` implementations and code constructing these messages must move to the new field names.

### Breaking
- **`SyncConfig` gains `max_request_bytes` and `max_clock_skew_ms`.** Struct literals without `..Default::default()` must add the new fields. (`sync` feature.)
- **New `SyncError::PayloadTooLarge` variant** — exhaustive matches on `SyncError` must handle it. (`sync` feature.)

## [0.7.0] - 2026-08-09

> **Sprint 4.3 — Substrate API Seams.** Three slices unblocking downstream consumer PulseBase: an embedding injection seam so embedded-Rust consumers drive embed-on-write, a provider identity that prevents cross-provider vector mixing on local write paths, and key-value tags with substrate-native filtered ANN search. Plus batch dependency bumps (thiserror 2, sha2 0.11, reqwest 0.13, tokio-tungstenite 0.29, criterion 0.8).

### Added
- **Key-value tags on experiences.** `NewExperience`/`Experience` now carry a `tags: BTreeMap<String,String>` field (default empty) for structured key-value filtering, orthogonal to the existing flat `domain: Vec<String>` categorical tags. Serialized in the postcard format; schema bumped v3→v4 with an on-open migration that appends an empty tags map to existing records. Closes #62.
- **Substrate-native tag-filtered ANN search.** `SearchFilter::tags_all` (exact-match subset: experience must have all given key=value pairs) is pushed **into the HNSW graph traversal** (filter-during-traversal), not applied as a post-filter. A search for `k` results among a tagged subset returns exactly `k` tagged results — not `k′ < k` after a post-recall truncate. Backed by a new `EXPERIENCES_BY_TAG` multimap index for O(matches) predicate resolution.
- **Embedding injection seam (`PulseDB::open_with_embedder`).** A downstream consumer can now drive embed-on-write through its own `impl EmbeddingService` instead of pre-computing vectors and passing `Some(vec)` on every write. Pass an `Arc<dyn EmbeddingService + Send + Sync>` at open; `record_experience`/`store_insight` with `embedding: None` route through it. The existing `PulseDB::open` path is unchanged. Unblocks embedded-Rust consumers (e.g. PulseBase). Closes #61.
- **Provider identity travels with the manifold.** Every `EmbeddingService` now declares a `ProviderIdentity` (provider name + model id) via a required `identity()` trait method. `PulseDB::provider_identity()` returns the identity stamped into the store's persisted metadata — so the open database reports which provider embedded its contents.
- **Cross-provider-mismatch prevention on local write paths.** Re-opening an existing manifold via `open_with_embedder` or `PulseDB::open` with an embedder whose persisted provider identity differs from the stored one is refused with a typed `PulseDBError::EmbeddingProviderMismatch`. This **prevents cross-provider mixing on the local write paths** (`record_experience`, `store_insight`) for distinct loaded model+tokenizer byte sets under `open_with_embedder` and `Builtin`-via-`open` stores. The identity is a construction-time full SHA-256 fingerprint of the loaded `model.onnx ‖ tokenizer.json` bytes (`onnx-<hash>`); the dimension is validated against the configured dimension before stamping. See Known Limitations for the honest scope.

### Changed
- **Batch dependency bumps** (PR #74): thiserror 1→2, sha2 0.10→0.11, reqwest 0.12→0.13, tokio-tungstenite 0.24→0.29, criterion 0.5→0.8, actions/cache 5→6, action-gh-release 2→3. Cargo-deny-action bumped 2.0.20→2.1.1 (PR #64).

### Breaking
- **Schema v3→v4 on-disk migration.** Experiences now carry a `tags: BTreeMap<String,String>` field; existing v3 stores migrate automatically on the first **writable** open (a `.pre-v4.bak` sidecar is claimed first). A read-only open of a not-yet-migrated store returns a typed error instead of migrating.
- **`SYNC_PROTOCOL_VERSION` 3→4.** The `Experience` blob and `SerializableExperienceUpdate` now include the `tags` field. Mixed-version sync peers fail loud in both directions — upgrade all peers together (`sync` / `sync-http` features only).
- **`EmbeddingService` trait gains a required `identity()` method.** Custom `impl EmbeddingService` must now implement `fn identity(&self) -> Result<ProviderIdentity>`. External-provider consumers using `Some(vec)` on every write are unaffected (the `External`-via-`open` path stays caller-controlled).
- **`ManagedEmbedderPresent` replaces `InjectedEmbedderPresent`.** All managed embedders (`open_with_embedder` AND `Builtin`-via-`open`) now refuse `Some(vec)` with a typed error; only `External`-via-`open` retains the per-record vector API.
- **New `PulseDBError` variants**: `EmbeddingProviderMismatch`, `ManagedEmbedderPresent`, `ProviderIdentityCorrupted` — exhaustive matches on `PulseDBError` must handle them.

### Known Limitations
- **Pre-0.7.0 stores trigger a one-time lenient identity adoption on first open** (when both `PROVIDER_IDENTITY_KEY` and `PROVIDER_IDENTITY_STAMPED_AT_KEY` are absent). A post-0.7.0 store whose stamp was lost or corrupted (era marker present but identity absent) is refused with a typed corruption error rather than silently re-adopted (VS-4.3.3 1.01, `pulsedb-internal` #17).
- **The sync replication path is peer-trusted and not identity-checked.** `apply_synced_experience`/`apply_synced_insight` write vectors directly into the HNSW index without checking the local provider identity, and the sync handshake does not exchange `ProviderIdentity`. A stamped store can therefore ingest vectors produced by an unrelated peer via sync. The prevention guarantee scopes to the **local write paths** only; a principled sync-handshake identity exchange is a deferred protocol-bump follow-up (`pulsedb-internal` #19).
- **Runtime pipeline configuration is not discriminated.** Two embedding functions sharing byte-identical `model.onnx` + `tokenizer.json` but differing in pooling, normalization, `max_length`, or ONNX Runtime version pass the guard. The identity discriminates distinct loaded **model+tokenizer byte sets**, not the full embedding pipeline. Pipeline-config fingerprinting is a follow-up (`pulsedb-internal` #18).
- **Custom injected embedders can return caller-chosen identity strings.** The guard compares the injected `EmbeddingService::identity()` strings; it cannot verify those strings truthfully describe the embedder's behavior. A malicious or buggy custom embedder can stamp any identity.
- **`External`-via-`open` remains caller-controlled.** `Some(vector)` is the only way `External` stores get populated; the legacy `open` + `External` + `Some(vec)` API (since v0.1.0) stays legal. All managed embedders (`open_with_embedder` AND `Builtin`-via-`open`) refuse `Some(vec)` with a typed `ManagedEmbedderPresent` error (VS-4.3.3 1.03 + PR #66 A1).
- **On platforms without redb file locking, concurrent writable opens are the caller's responsibility** per redb's contract (`pulsedb-internal` #11). The check-then-set stamp shape is serialized by redb 4.1's exclusive writable file lock on supported platforms.
- **A re-exported model gets a different identity.** A user who re-exports the same ONNX model (producing different `model.onnx` bytes) gets a different `onnx-<hash>` identity and the guard refuses the reopen. The one-time `{builtin-onnx, main_graph}` → `{builtin-onnx, onnx-<hash>}` migration (VS-4.3.3 1.04) handles the bundled-MiniLM case automatically; non-MiniLM stores require consumer-side re-embed.

### Fixed
- **Migration backup sidecar is now crash-atomic.** `backup_once` stages the pristine `.pre-substrate.bak` at a temp path and publishes it via an atomic rename after the `sync_all`, so an abrupt process death mid-copy leaves the final sidecar **absent** — a clean retry re-copies a fresh pristine backup — instead of a truncated file a later open could preserve and trust as a valid rollback point.
- **A lock-aborted redb v2→v3 upgrade no longer leaves a stale backup sidecar.** When the destructive upgrade aborts because a legacy writer still holds the file (`DatabaseLocked`; the store is untouched), a `.pre-substrate.bak` *written by that attempt* — which may be a stale snapshot — is removed so the retry re-backs-up a fresh copy. A sidecar preserved from an earlier attempt, or one left by a torn in-place upgrade, is kept as the rollback point.
- **Windows lock classification is scoped to redb-file operations.** The `ERROR_SHARING_VIOLATION` / `ERROR_LOCK_VIOLATION` → `DatabaseLocked` mapping now applies only to the ops that read the store file during backup; a transient sharing violation on the backup sidecar/temp itself (e.g. an antivirus/indexer touching it) surfaces as a plain I/O error rather than a spurious retryable lock.

### Security
- **Backup staging is symlink-safe.** `backup_once` unlinks any pre-existing entry at its temp staging path and creates the file with `O_EXCL`, so a symlink pre-planted there (by another local user with write access to the database's parent directory) is never followed to write through to an attacker-chosen target.

## [0.6.0] - 2026-07-05

> **Sprint 4.0 — Storage-Format Modernization.** Adopts the redb 2.x→4.x on-disk file-format major and replaces the unmaintained `bincode` serializer with `postcard`, both behind a tested upgrade-on-open path. A database from a prior release (v0.5.1 / v0.4.0) opens under the new format and reads back identically — verified against **real prior-release on-disk stores** (NFR-020), plus a kill-at-boundary crash-recovery gate.

### Changed
- **On-disk storage format modernized to redb 4.x + postcard.** The redb file format is upgraded v2→v3 in place and every serde-blob value is re-encoded from bincode to postcard (`SUBSTRATE_FORMAT` marker 1→2), both on the first writable open. Embeddings, secondary indexes, and raw metadata keys are copied through byte-identically. See **[docs/storage-migration.md](docs/storage-migration.md)** for the migration & crash-recovery posture — the disk+memory headroom preflight, the single-transaction "re-run from scratch on crash" contract, and the large-store fail-closed behavior.

### Added
- **Real prior-release upgrade gate** (`tests/storage_format_upgrade.rs`): two curated real fixtures (v0.5.1 schema-v3, v0.4.0 schema-v2) migrate value- and byte-identical against a SHA-256-provenanced manifest oracle (NFR-020 / MIGRATE-020). Sampled compatibility with documented residuals (v0.3.0 / WAL-v1 uncovered).
- **Kill-at-boundary crash-recovery suite** (`--features fault-injection`, a Linux CI gate): a compiled-out injection seam crashes the migration at all five boundaries plus one subprocess SIGKILL, asserting positive recovery (atomicity ≠ resumability).
- **Migration headroom preflight**: disk + memory axes are validated before any destructive write; an above-floor store fails closed with a typed, actionable error rather than risking an unfinishable migration. New `Config::migration_available_memory_bytes` opts into a higher single-transaction ceiling.
- New typed `StorageError` variants: `SubstrateUpgradeRequired`, `SubstrateFormatTooNew`, `SubstrateMigrationTooLarge`, `SubstrateMigrationInsufficientDisk`, `SubstrateMigrationRequiresSync`.

### Removed
- **Dropped the unmaintained `bincode` crate dependency** (RUSTSEC-2025-0141) and its `deny.toml` advisory ignore — storage now uses postcard plus a vendored decode-only bincode-1.3 reader (`storage::legacy_bincode`) for the one-time legacy-data migration (ADR-006, Accepted; see [docs/adr/ADR-006-serializer-replacement.md](docs/adr/ADR-006-serializer-replacement.md)). `cargo deny check --all-features` is green with no advisory ignores.

### Breaking
- **On-disk format upgrade required.** Databases created by v0.5.1 and earlier are redb-v2 / bincode; they upgrade automatically on the first **writable** open (a `.pre-substrate.bak` sidecar is claimed first). A read-only open of a not-yet-migrated store returns a typed error instead of migrating.
- **New `StorageError` variants** (see Added) — exhaustive matches on `StorageError` must handle them.
- **sync-http wire format**: a serializer-independent preamble is read before the handshake body and `SYNC_PROTOCOL_VERSION` bumps 2→3; mixed-version peers fail loud in both directions — upgrade all peers together (`sync-http` feature only).

## [0.5.1] - 2026-06-20

> **Public-boundary hardening.** Metadata, licensing, and security-posture fixes only — no `src/`, API, or behavior changes.

### Fixed
- **Crate metadata**: corrected the `repository` URL to `https://github.com/pulseai-labs/PulseDB` (was a non-existent org) and the README CI badge to match. The crates.io / docs.rs "Repository" link now resolves.

### Added
- `LICENSING.md` — documents the AGPL-3.0 + commercial dual-license posture and the commercial-license contact.
- `SECURITY.md` — responsible-disclosure policy (private vulnerability reporting is enabled on the repo).
- `PUBLIC_BOUNDARY.md` — what is public vs. internal for the PulseDB open-source repo.
- `CONTRIBUTING.md` — contribution guidelines.
- `.gitignore` secret-file patterns (`*.pem`, `*.key`, `*.crt`, `id_rsa`, `.secrets/`, …).
- Security CI: `cargo-deny` job (advisories/bans/sources); Dependabot config (`cargo` + `github-actions`). Secret scanning is enforced via GitHub-native secret scanning + push protection (`.gitleaks.toml` is kept for optional local scans).
- CI hardening: least-privilege default `GITHUB_TOKEN` permissions; the publish job now runs through a manual-approval `crates-io` environment.

## [0.5.0] - 2026-06-20

> **Sprint 3.5 — Temporal Dynamics.** Three vertical slices: decay core + schema v3 (VS-3.5.1), energy-weighted recall (VS-3.5.2), lifecycle surfacing + 1M bench guard (VS-3.5.3).

### Added

#### Temporal energy & decay (VS-3.5.1)
- `PulseDB::energy(id) -> f32` — temporal-energy diagnostic for an experience, derived-on-read (never stored): `E = clamp(importance · (1 + freq_weight · ln(1 + applications)) · exp(−ln2 · Δt / half_life), 0, 1)`.
- `PulseDB::reinforce_experience(id) -> u32` — reinforcement now increments the local instance's bucket in a per-instance G-counter and returns the new total application count (CRDT-safe across instances).
- `Experience::applications() -> u32` — total application count summed across all instance buckets.
- `DecayConfig` — per-collective decay configuration: `half_life` (default 30 days), `freq_weight` (`k` in `1 + k·ln(1 + applications)`, default 0.25), `floor` (cold threshold, default 0.05), `auto_archive_below_floor` (default `false`), `default_recall_weights` (default `None`). Configured via `Config.decay`.
- `SubstrateProvider::reinforce_experience()` and `SubstrateProvider::energy()` — async substrate surfaces (trait default returns unsupported-operation; `PulseDBSubstrate` delegates to the blocking core; backward compatible).
- Exact G-counter merge for `applications` under sync — bidirectional replication converges via per-instance max, with no lost or doubled increments (assumes a distinct `InstanceId` per replica; see #10).

#### Energy-weighted recall (VS-3.5.2)
- `RecallWeights { similarity, energy }` (with `RecallWeights::new`) — blend weights for energy-aware ranking.
- `SearchOptions.weights: Option<RecallWeights>` — opt-in energy-weighted ranking on `PulseDB::search`. Default `None` preserves the legacy pure-similarity path **byte-for-byte** (as does `{ similarity: 1.0, energy: 0.0 }`).
- `DecayConfig.default_recall_weights: Option<RecallWeights>` — per-collective default blend, resolved per query (invalid stored weights are ignored; see #16).
- `get_context_candidates` honors recall weights for energy-aware context retrieval.
- Ranking blends `similarity·sim + energy·E` over the cosine top-`k′` candidate frontier (over-fetch-then-re-rank): energy reorders admitted candidates but does not itself retrieve high-energy/low-similarity records (known limitation; see #15).

#### Temporal lifecycle surface (VS-3.5.3)
- `PulseDB::list_cold_experiences(collective_id, below, limit)` — read-only, coldest-first surfacing of prune-eligible cold experiences. Returns lightweight `(ExperienceId, energy)` pairs (not full `Experience` records) for experiences whose current temporal energy is `< below` and that are not already archived. A human/agent-triggered review tool: it surfaces candidates a consumer may choose to archive, but never mutates storage. `below` ∈ `[0.0, 1.0]`, `limit` ∈ `1..=1000`; deliberate `O(n)` full-collective scan.
- `SubstrateProvider::list_cold_experiences()` — async substrate mirror of the cold-experience surfacing API, with a trait default (unsupported-operation) and a `PulseDBSubstrate` override that delegates to the blocking core (backward compatible).

#### Performance guard (VS-3.5.3)
- NFR-018 1M P99 search-latency criterion bench guard (`cargo bench search`) — the 1M-experience P99 search latency is measured and recorded against the 50 ms budget. Verdict: **MET @ 9.35 ms** (~5.3x headroom). The guard prints the measured P99 and does not panic on regression (records the verdict for review; no forward CI enforcement yet — see #19).

### Changed
- **BREAKING — schema v3.** The on-disk schema bumps to v3 with an automatic, one-time `v1/v2 → v3` migration on `open()` (a `.pre-v3.bak` sidecar is retained; read-only databases refuse the migration). `Experience` is reshaped: the former scalar reinforcement counter is replaced by a per-instance G-counter `applications: BTreeMap<InstanceId, u32>` (totalled via `Experience::applications()`), and a `last_reinforced: Timestamp` field is added. Code that constructed or pattern-matched `Experience` directly, or read the old scalar counter, must migrate to the new fields.

### Notes
- `auto_archive_below_floor` ships **inert** (default OFF): the flag round-trips through config but wires **no** automatic archive trigger. `list_cold_experiences` only surfaces candidates; no auto-archive actuator exists (rustdoc follow-up: #22).
- Per-collective `DecayConfig` is **local and unsynced** by design — energy is advisory/derived-on-read and may legitimately differ across replicas (DECAY_SPEC D4).

## [0.4.0] - 2026-03-26

### Added

#### PulseVision-ready APIs (Issue #8)
- `Config::read_only()` constructor and `read_only` field — opens database in read-only mode where all mutations return `PulseDBError::ReadOnly`
- `PulseDB::is_read_only()` method
- `PulseDBError::ReadOnly` variant with `is_read_only()` predicate
- `PulseDB::list_experiences(collective_id, limit, offset)` — paginated experience enumeration with embeddings
- `PulseDB::list_relations(collective_id, limit, offset)` — paginated relation listing
- `PulseDB::list_insights(collective_id, limit, offset)` — paginated insight listing
- `SubstrateProvider::list_experiences()`, `list_relations()`, `list_insights()` with default implementations (backward compatible)
- `WatchEvent.experience: Option<Experience>` — enriched events include full experience data for Created/Updated events (embeddings, importance, domain)

### Changed
- `WatchEvent` struct now has an `experience` field (`Option<Experience>`) — set to `Some` for Created/Updated events via in-process watch, `None` for Deleted and WAL-reconstructed events

## [0.3.0] - 2026-03-26

### Added

#### Native Sync Protocol
- `SyncManager` for orchestrating sync between PulseDB instances (start/stop/sync_once/initial_sync)
- `SyncTransport` pluggable trait for transport abstraction
- `HttpSyncTransport` for HTTP/HTTPS sync via reqwest (`sync-http` feature)
- `SyncServer` framework-agnostic server handler for Axum/other consumers (`sync-http` feature)
- `InMemorySyncTransport` for testing
- `SyncConfig` with direction (push/pull/bidirectional), conflict resolution (ServerWins/LastWriteWins), retry with exponential backoff
- `SyncApplyGuard` thread-local echo prevention (prevents infinite sync loops)
- `SyncProgressCallback` trait for initial sync UI feedback
- WAL extension: all entity types (experiences, relations, insights, collectives) now tracked in WAL
- Schema v2 migration (automatic on open)
- `PulseDB::compact_wal()` for WAL compaction using min-cursor strategy
- Per-peer sync cursor persistence in redb
- Stable `InstanceId` per database (UUID v7, persisted in metadata)
- `PulseDBError::Sync` variant (feature-gated)

#### Feature Flags
- `sync` — Core sync protocol, types, engine, in-memory transport
- `sync-http` — HTTP transport (reqwest) + server handler
- `sync-websocket` — WebSocket transport placeholder (tokio-tungstenite)

#### Testing & Benchmarks
- 65+ sync-specific integration tests (foundation, engine, HTTP)
- 6 Criterion benchmarks for sync operations (serialization, echo prevention, WAL poll, compaction)

### Changed
- WAL schema version 1 → 2 (entity_type field added to WatchEventRecord, auto-migration on open)
- `WatchEventRecord.experience_id` renamed to `entity_id` with new `entity_type` discriminant
- `poll_changes()` now filters to Experience-only events (backward compatible)
- WAL sequence now increments for relation, insight, and collective mutations

## [0.2.1] - 2026-03-19

### Fixed
- Race condition in builtin embedding model auto-download when multiple PulseDB instances open concurrently (file lock with double-check pattern)

## [0.2.0] - 2026-03-18

### Added
- `SubstrateProvider::create_collective()` for creating collectives through the async trait
- `SubstrateProvider::get_or_create_collective()` for idempotent collective creation (recommended for SDK consumers)
- `SubstrateProvider::list_collectives()` for listing all collectives
- Auto-download of builtin embedding model when missing (no manual download step needed)

### Breaking
- `SubstrateProvider` trait has 3 new required methods — implementors must add them

## [0.1.1] - 2026-03-15

### Changed
- Improved public documentation for docs.rs readability
- Added docs.rs build configuration for feature-gated items
- Added Feature Flags documentation table to crate-level docs

## [0.1.0] - 2026-03-15

### Added

#### Core
- Database open/close lifecycle with ACID guarantees via redb
- redb storage layer with schema versioning and corruption detection
- Collective CRUD operations for project-level isolation
- Experience CRUD (record, get, update, archive, delete, reinforce)
- Comprehensive input validation for all public APIs
- Built-in ONNX embedding service (all-MiniLM-L6-v2, 384d) with atomic model download (`builtin-embeddings` feature)

#### Search & Retrieval
- HNSW vector index integration for approximate nearest neighbor search (hnsw_rs)
- Similarity search API with cosine distance scoring and domain/type/importance filtering
- Recent experiences API with timestamp-ordered retrieval
- Unified context candidates API aggregating similar, recent, insights, relations, and active agents

#### Knowledge Graph
- Typed experience relations (Supports, Contradicts, Elaborates, Supersedes, Implies, RelatedTo)
- Direction-based relation querying (Outgoing, Incoming, Both)
- Derived insight storage with vector search
- Agent activity tracking with heartbeat and stale detection

#### Real-time & Integration
- In-process watch system for real-time experience notifications via crossbeam channels
- Cross-process change detection via WAL sequence tracking and file lock coordination
- Configurable watch behavior (WatchConfig: in_process toggle, poll interval, buffer size)
- SubstrateProvider async trait and PulseDBSubstrate adapter for agent framework integration

#### Quality
- Error handling audit: comprehensive PulseDBError hierarchy with actionable messages
- All public APIs documented with examples (50 doc tests passing)
- Property-based tests with proptest (7 invariant tests)
- Fuzz testing infrastructure with 3 cargo-fuzz targets
- Test coverage at 89.56% (2033/2270 lines)
- Criterion benchmarks for core operations, mixed workloads, and scaling (1K-100K)
- CI pipeline: 6 jobs (lint, test, MSRV, coverage, security audit, benchmarks)
- CI regression detection with critcmp (10% threshold)

### Performance Targets

| Operation | Target | Measured (1K) |
|-----------|--------|---------------|
| `record_experience` | < 10 ms | 5.5 ms |
| `search_similar` (k=20) | < 50 ms | 95 us |
| `get_context_candidates` | < 100 ms | 189 us |
| `open()` | < 100 ms | < 5 ms |
