# Storage migration & crash-recovery posture

> Applies to the one-time **upgrade-on-open** migration that runs when an older PulseDB store is opened
> by a newer binary. Covers the redb file-format upgrade (v2→v3) and the value-codec cutover
> (bincode→postcard, `SUBSTRATE_FORMAT` marker 1→2). Steady-state opens are unaffected.

## What happens on first open of an older store

When `PulseDB::open()` (writable) encounters an older on-disk store, it performs a one-time migration
**inside the open call**:

1. **Headroom preflight (before any destructive write).** A unified check covering two axes:
   - **Disk** — free space on the store's filesystem must cover the pristine backup (~1× the store size)
     plus the migrated file (~1× — the postcard re-encode does **not** shrink the size-dominant raw-`f32`
     embedding table) plus a transaction-growth margin. If short, the open fails with a typed
     `SubstrateMigrationInsufficientDisk` error and **zero writes**.
   - **Memory** — the projected single-transaction peak (≈ `0.10 × store_size`, measured) is checked
     **config-first** against a conservative store-size floor (1 GiB by default) or, when the embedder
     declares `Config::migration_available_memory_bytes`, against that declared budget. Host memory is
     **not** auto-detected (a cgroup-limited container over-reports RAM and would OOM). If the store is
     above the floor with no covering declared budget, the open fails with a typed
     `SubstrateMigrationTooLarge` error and **zero writes**.

   Both axes fail **closed** before any destructive write — never a half-migration that runs the machine
   out of disk or memory mid-pass.

2. **Pristine backup.** Before any migration write, the original file is copied to
   `<db>.pre-substrate.bak` (atomic, create-once). This single pristine backup precedes **both** the
   redb-format and the codec migrations, so it is the rollback point for the whole window.

3. **Migration (single write transaction).** The redb format upgrade (v2→v3) and the codec re-encode
   (every serde-blob value bincode→postcard; raw-byte tables — embeddings, secondary indexes, raw
   metadata keys — copied through byte-identically) run, and the `SUBSTRATE_FORMAT` marker is bumped to
   `2` as the **last write in the same transaction** (the atomic commit point).

4. **Progress signal.** The migration emits a one-time "this may take a while" log line plus per-phase
   `info!` progress, so a multi-minute migration of a large store is not a silent hang. The first-open
   migration is **explicitly exempt from NFR-001** (`<100ms` open); steady-state opens still meet it.

## Crash-recovery posture

The migration is realized as a **single write transaction** (the common path — measurements show
essentially all real stores fit the single-txn memory budget). Its crash-recovery contract:

- **Crash during the single-txn migration → re-run from scratch.** Because the marker bump is the last
  write in the transaction, a crash before commit leaves the **old marker and old-codec-decodable data
  intact** (redb rolls the uncommitted transaction back). On the next open the migration simply
  **re-runs from scratch** against the still-pristine `.pre-substrate.bak` rollback point. This is
  **safe, not fast**: a crashed migration is re-done in full, not resumed mid-way.

- **Store above the single-txn floor with no declared memory budget → fail closed.** Such a store
  currently refuses to open (typed `SubstrateMigrationTooLarge`) rather than risk an OOM that never
  finishes. To migrate it, either declare available memory via `Config::migration_available_memory_bytes`
  (opting into a single-txn migration the host can hold) or use the **offline migration tool** (below).
  A **resumable phased migration** for this case is **not yet implemented** (planned — see *Deferred*).

- **Restore from backup.** If a migrated store is ever suspect, the pristine `<db>.pre-substrate.bak`
  is a byte-for-byte snapshot of the pre-migration file and can be restored manually.

## Offline migration tool (`pulsedb migrate`)

For very large stores — especially the above-floor / undeclared-memory case — the recommended path is an
**offline `pulsedb migrate` tool** that performs the upgrade out of the hot open path with explicit
resource control. **Status: deferred** (planned for a later release; tracked with the large-store
safeguards). Until it ships, large stores migrate by declaring a memory budget, or by running on a host
that clears the single-txn budget.

## Deferred

- **Resumable phased migration** (per-table durable progress markers) for above-floor / undeclared-memory
  stores — deferred (tracked as crash-recovery / resumability follow-up #46).
- **Kill-at-every-boundary fault-injection tests** — deferred to the real-fixture hardening slice (#46).
- **Offline `pulsedb migrate` tool** implementation — deferred (#45).
