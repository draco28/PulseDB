# ADR-011: Rollback via SemVer Major Bumps and Backup-Before-Migrate

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB needs a rollback strategy for bad releases and schema migration paths.

## Decision
PulseDB uses SemVer major bumps for breaking changes. For storage format changes (redb major versions, serializer changes), a backup-before-migrate strategy is used: `.pre-substrate.bak` is written before any destructive migration. Rollback means reinstalling the previous version and restoring from backup. The migration is idempotent — reopening a migrated store re-runs migration safely.

## Touch surface
`src/storage/redb.rs`, `src/lib.rs`, `Cargo.toml`

## Revisit trigger
Revisit when rollback strategy needs updating (e.g., point-in-time recovery).
