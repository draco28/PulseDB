---
name: qa-library
description: >
  Consumer-simulation QA tests for the PulseDB Rust library crate. Exercises the
  public API (open, create collective, record experience, search, insights, sync,
  provider identity) as a real downstream consumer would. Uses cargo test to run
  small Rust integration programs. Covers all 5 feature configurations.
---

# QA Sub-Skill — PulseDB Library (consumer simulation)

This sub-skill defines the menu of consumer-simulation test flows for the `pulsedb`
crate. The orchestrator (qa/SKILL.md) selects flows based on the diff and runs them
by writing small `tests/qa_flow_*.rs` files, compiling, and running with `cargo test`.

## Prerequisites

- Rust toolchain (`rustc`, `cargo`)
- The checked-out PR branch (NOT a published crate version)
- `tempfile` is a dev-dependency (already present)
- For `builtin-embeddings` tests: the bundled MiniLM model cached or auto-downloadable

## Test Flow Menu

The orchestrator picks flows relevant to the diff. Each flow below is a template for
a `tests/qa_flow_*.rs` file.

### Flow 1: Core Lifecycle (default features)

**Trigger:** changes in `src/db.rs`, `src/storage/**`, `src/config.rs`, `src/collective/**`
**Feature:** default

Exercises the fundamental lifecycle a consumer performs: open → create collective →
record experience → search → close → reopen → verify persistence.

### Flow 2: Embedding Injection Seam (open_with_embedder)

**Trigger:** changes in `src/db.rs` (open_with_embedder), `src/embedding/**`
**Feature:** default (stub embedder); builtin-embeddings for OnnxEmbedding

Exercises the injected-embedder constructor: open with custom embedder → record with
None → verify embedding routes through injected service → provider_identity persists
across reopen → mismatch refusal with different identity.

### Flow 3: Cross-Provider Prevention

**Trigger:** changes in `src/db.rs` (stamp/mismatch/era-marker), `src/embedding/**`,
`src/error.rs`, `src/storage/schema.rs`
**Feature:** default (External tests) + `--features builtin-embeddings` (OnnxEmbedding tests)

Exercises the cross-provider-mismatch guard on BOTH constructors: stamp identity A →
reopen with different config → refused with EmbeddingProviderMismatch; Some(vec) under
open_with_embedder → refused with InjectedEmbedderPresent; era-marker-present +
identity-absent → corruption error on both constructors.

### Flow 4: Search + Ranking

**Trigger:** changes in `src/search/**`, `src/experience/**`
**Feature:** default

Exercises vector search with filters and ranking: record multiple experiences with
different embeddings → search → verify results ordered by similarity.

### Flow 5: Insights + Relations

**Trigger:** changes in `src/insight/**`, `src/relation/**`
**Feature:** default

Exercises insight creation + relation linking: record experience → store insight →
search insights → verify searchable.

### Flow 6: Sync Replication (feature-gated)

**Trigger:** changes in `src/sync/**`
**Feature:** `--features sync` (or sync-http for HTTP transport)

Exercises two-instance sync convergence: record in A → sync → verify in B.

### Flow 7: Decay + Lifecycle (cold experiences)

**Trigger:** changes in `src/experience/**` (decay, lifecycle)
**Feature:** default

Exercises temporal decay + cold-experience listing: record → energy(id) →
list_cold_experiences.

## Known Failure Modes

1. **Model not cached.** `builtin-embeddings` tests that construct `OnnxEmbedding` may
   fail if the model is not cached. The download happens automatically on first run but
   may timeout in CI. Report BLOCKED if download fails.

2. **Feature flag mismatch.** A flow that uses `open_with_embedder` must NOT run with
   `--features builtin-embeddings` unless the embedder is OnnxEmbedding.

3. **TempDir cleanup on panic.** If a test panics, `TempDir` is not cleaned up on some
   platforms. This is harmless but may leave `.db` files in `/tmp`.

4. **redb file lock.** If a previous test process crashed, redb's exclusive lock may
   prevent a new open. Use unique `TempDir` paths per test (the default).

5. **Search dimension mismatch.** `search_similar` requires the query vector to match
   the collective's configured dimension. Default is 384.
