# fixture-gen-v0_4_0 — real `pulsehive-db =0.4.0` (schema-v2) golden-fixture generator

Generates the **real prior-release** on-disk store `tests/fixtures/real-v0.4.0.redb`
+ its provenance manifest `tests/fixtures/real-v0.4.0.manifest.json`, built by the
**published** `pulsehive-db =0.4.0` crate resolved from crates.io — NOT synthesized
in-tree. This is the schema-v2 half of VS-4.0.4's discharge of **NFR-020**; it
exercises **all three** migration axes (the v0.5.1 fixture in `../fixture-gen-v0_5_1`
covers only axes 1+2).

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
- It is built **only** via an explicit `--manifest-path tools/fixture-gen-v0_4_0/Cargo.toml`
  (see `regen-fixtures.sh`), and its build `target/` defaults **outside** the repo.
- The production root `Cargo.toml` is a single `[package]` manifest with **NO
  `[workspace]` section and NO `exclude` entry**, and it stays that way — a
  `cargo metadata --no-deps` on the root never lists `fixture-gen*`.
- `pulsehive-db` is pinned **exactly** (`=0.4.0`) so the generator resolves the
  published crates.io artifact — never a local path override. Default features only
  (no `builtin-embeddings` → no ONNX/`ort`); embeddings are injected as raw f32
  vectors via the **External** embedding provider.

## Shape coverage (v0.4.0, schema-v2)

Every serde-blob entity the published public API can produce:
- **collectives** (incl. an owner via `create_collective_with_owner`)
- **experiences** across every `ExperienceType` variant, each with a **384-d
  embedding** (populates the raw-f32 `embeddings` table + the
  `experiences_by_collective` / `experiences_by_type` secondary multimap indexes)
- **experience-relations** (`store_relation`)
- **derived insights** (`store_insight`)
- **watch events** (captured via `poll_changes`)
- **db_metadata** (schema_version = 2, embedding_dimension)

**No decay surface:** v0.4.0 predates decay — there is no `DecayConfig` / `decay`
field in the 0.4.0 public API. Recorded as an explicit absence, not a gap.

**`instance_id` is ABSENT** in a default-features schema-v2 store: v0.4.0 persists
`instance_id` **only** behind the `sync` feature. A fresh `instance_id` is **minted**
during the v2→v3 migration — so 4.02 must assert an instance_id now *exists*
(minted), not a specific value. (Contrast the v0.5.1 fixture, whose instance_id
migration must *preserve*.)

### Migration axes exercised
v0.4.0 is schema-v2, so this fixture exercises **ALL THREE** axes: **axis-1** (redb
file-format v2→v3), **axis-2** (bincode→postcard value re-encode), and **axis-3**
(the v2→v3 logical reshape — `migrate_experiences_v2_to_v3` + `migrate_wal_v1_to_v2`).

## Manifest = mechanical provenance + ground truth

Same structure as the v0.5.1 fixture:
- **Provenance (audit C1):** `blob_sha256` (recomputed + compared by 4.01's AUTO
  provenance AC), `generator_git_commit`, `generator_cargo_lock_sha256`,
  `resolved_dependency_checksums`, `feature_set`, `build_env`.
- **Value + raw-bytes ground truth (audit C4):** typed read-back values PLUS the
  **genuine on-disk raw bytes** of the copy-through tables, read via redb 2.6
  directly.
- **Search ground truth, NOT HNSW internals (audit C14):** fixed query + expected
  top-k; HNSW internals are not captured (rebuilt on open — issue #18).
- **Falsification contract (audit C10):** corrupt/truncated blob must fail loudly —
  `blob_sha256` here + 4.02's corrupt-fixture negative test.

## Documented coverage gaps (not faked)

- **`SyncCursor`**: not crate-root re-exported, behind the `sync` feature; public
  constructability uncertain → manifest coverage note, **not** a fabricated blob.
- **Residual v0.3.0 / WAL-v1** (the logical schema BEFORE v2): not synthesized —
  v0.4.0 is already schema-v2 (WAL-v2), so a genuine WAL-v1 / v0.3.0 artifact is out
  of scope and left as a **documented residual gap**, not faked.
