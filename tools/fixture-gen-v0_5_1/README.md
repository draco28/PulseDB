# fixture-gen-v0_5_1 — real `pulsehive-db =0.5.1` (schema-v3) golden-fixture generator

Generates the **real prior-release** on-disk store `tests/fixtures/real-v0.5.1.redb`
+ its provenance manifest `tests/fixtures/real-v0.5.1.manifest.json`, built by the
**published** `pulsehive-db =0.5.1` crate resolved from crates.io — NOT synthesized
in-tree. This is one half of VS-4.0.4's discharge of **NFR-020** (a prior-release DB
opens under the new format and reads back identically); the other half is the
schema-v2 `real-v0.4.0` fixture (`../fixture-gen-v0_4_0`).

## Regenerate

```bash
# from anywhere in the repo — regenerates BOTH fixtures + manifests:
tools/regen-fixtures.sh
```

Regeneration is **reproducible-with-drift**: each run mints new UUIDv7 ids +
timestamps, so it produces a **new `blob_sha256`**. The committed `.redb` blob and
its manifest are a matched, **frozen** pair — regen is for provenance/audit, not a
byte-for-byte reproduction. The generator is committed for provenance and is **NOT**
run in CI; 4.02 runs the upgrade test against the frozen blob.

## Build isolation (audit C9) — OUTSIDE the production workspace

- This crate's `Cargo.toml` carries an **empty `[workspace]` table**, making it its
  own workspace root. Cargo never walks up into the production root manifest.
- It is built **only** via an explicit `--manifest-path tools/fixture-gen-v0_5_1/Cargo.toml`
  (see `regen-fixtures.sh`), and its build `target/` defaults **outside** the repo.
- The production root `Cargo.toml` is a single `[package]` manifest with **NO
  `[workspace]` section and NO `exclude` entry**, and it stays that way — a
  `cargo metadata --no-deps` on the root never lists `fixture-gen*`.
- `pulsehive-db` is pinned **exactly** (`=0.5.1`) so the generator resolves the
  published crates.io artifact — never a local path override. Default features only
  (no `builtin-embeddings` → no ONNX/`ort`); embeddings are injected as raw f32
  vectors via the **External** embedding provider.

## Shape coverage (v0.5.1, schema-v3)

Every serde-blob entity the published public API can produce:
- **collectives** (incl. an owner via `create_collective_with_owner`)
- **experiences** across every `ExperienceType` variant, each with a **384-d
  embedding** (populates the raw-f32 `embeddings` table + the
  `experiences_by_collective` / `experiences_by_type` secondary multimap indexes)
- **experience-relations** (`store_relation`)
- **derived insights** (`store_insight`)
- **watch events** (captured via `poll_changes`)
- **decay config** — v0.5.1 has the decay surface; recorded as the global
  `Config.decay` (the per-collective `decay_configs` table has **no public setter**
  in 0.5.1, so that table is intentionally unpopulated — documented in the manifest)
- **db_metadata** (schema_version, embedding_dimension)

`instance_id` **is** present in a v0.5.1 store (persisted unconditionally in
schema-v3) and is recorded — migration to v3 must **preserve** it (contrast the
v0.4.0 fixture, where it is minted on migration).

### Migration axes exercised
v0.5.1 is already schema-v3, so this fixture exercises **axis-1** (redb file-format
v2→v3) + **axis-2** (bincode→postcard value re-encode) but **not** axis-3 (the
v2→v3 logical reshape) — that is the `real-v0.4.0` fixture's job.

## Manifest = mechanical provenance + ground truth

- **Provenance (audit C1):** `blob_sha256` (recomputed + compared by 4.01's AUTO
  provenance AC), `generator_git_commit`, `generator_cargo_lock_sha256`,
  `resolved_dependency_checksums` (from `Cargo.lock`), `feature_set`, `build_env`
  (os/arch/rustc/toolchain).
- **Value + raw-bytes ground truth (audit C4):** typed read-back values for every
  entity, PLUS the **genuine on-disk raw bytes** of the copy-through tables
  (`embeddings` + `experiences_by_collective` / `experiences_by_type`), read back via
  redb 2.6 directly — so 4.02 can assert genuine copy-through **byte**-identity.
- **Search ground truth, NOT HNSW internals (audit C14):** a fixed query embedding +
  the expected top-k experience ids/similarities from `search_similar`. HNSW index
  internals are **not** captured (rebuilt from redb on every open — issue #18).
- **Falsification contract (audit C10):** a corrupted/truncated blob must fail
  loudly downstream — `blob_sha256` here + 4.02's corrupt-fixture negative test.

## Documented coverage gaps (not faked)

- **`SyncCursor`** (`src/sync/types.rs`): not crate-root re-exported, lives behind
  the `sync` feature; public constructability is uncertain → a manifest coverage
  note, **not** a fabricated blob.
- **Residual v0.3.0 / WAL-v1** (logical schema before v2): not synthesized — a
  documented residual gap, not closed here.
