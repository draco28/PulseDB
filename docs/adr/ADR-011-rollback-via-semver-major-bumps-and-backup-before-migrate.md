# ADR-011: Rollback via SemVer Major Bumps and Backup-Before-Migrate

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB needs a rollback strategy for bad releases and schema migration paths.

## Decision
Versioning follows ADR-008: while pre-1.0 (now), **breaking changes bump MINOR**; after 1.0, breaking changes bump MAJOR. (One policy — this ADR defers to ADR-008 on pre-1.0 levels.) For storage format changes (redb major versions, serializer changes), a backup-before-migrate strategy is used: `.pre-substrate.bak` is written before any destructive migration. **Known exception:** the codec-only leg (an already-redb-v3 store still carrying the bincode-era substrate marker) re-encodes to postcard *without* writing a backup — by then the previous binary cannot read the rewritten values either, so `.pre-substrate.bak` restore is not the rollback for that leg; the operator's rollback there is restoring from their own external backup or re-creating from a pre-upgrade copy. Rollback means reinstalling the previous version and restoring from backup. The migration is idempotent — reopening a migrated store re-runs migration safely.

## Touch surface
`src/storage/redb.rs`, `src/lib.rs`, `Cargo.toml`

## Revisit trigger
Revisit when rollback strategy needs updating (e.g., point-in-time recovery).
