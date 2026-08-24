# PulseDB Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) documenting key technical decisions made during PulseDB development.

## ADR Index

| ADR | Title | Status | Date | Summary |
|-----|-------|--------|------|---------|
| [ADR-001](ADR-001-redb-for-storage.md) | Use redb for Storage | Accepted | 2026-02-01 | Pure Rust embedded KV store with ACID and MVCC |
| [ADR-002](ADR-002-hnswlib-for-vector-index.md) | Use hnswlib for Vector Index | **Superseded** | 2026-02-01 | Superseded by ADR-005 |
| [ADR-003](ADR-003-single-writer-concurrency.md) | Single-Writer Concurrency | Accepted | 2026-02-01 | SWMR model matching redb semantics |
| [ADR-004](ADR-004-rich-experience-types.md) | Rich ExperienceType (9 variants) | Accepted | 2026-02-13 | 9 structured variants from Data Model over simplified 6 |
| [ADR-005](ADR-005-pure-rust-hnsw.md) | Pure Rust HNSW via hnsw_rs | Accepted | 2026-02-14 | Replace C++ hnswlib FFI with pure Rust hnsw_rs + VectorIndex trait |
| [ADR-006](ADR-006-serializer-replacement.md) | Serializer selection — postcard | Accepted | 2026-06-30 | Replace unmaintained bincode with postcard for disk + sync wire |
| [ADR-007](ADR-007-embedded-single-process-database-library.md) | Embedded single-process library | Accepted | 2026-08-23 | Deployment unit is the crate; cross-process access serialized |
| [ADR-008](ADR-008-public-api-semver-compatibility-policy.md) | Public API SemVer policy | Accepted | 2026-08-23 | Pre-1.0 breaking = MINOR; boundary = exported items |
| [ADR-009](ADR-009-trust-boundary-file-system-only.md) | Trust boundary: FS + public API | Accepted | 2026-08-23 | FS-only on default builds; sync-http adds a wire boundary |
| [ADR-010](ADR-010-failure-visibility-via-tracing-crate.md) | Failure visibility via tracing | Accepted | 2026-08-23 | Instrumented spans; error-level events not systematic |
| [ADR-011](ADR-011-rollback-via-semver-major-bumps-and-backup-before-migrate.md) | Rollback via backup-before-migrate | Accepted | 2026-08-23 | Format changes back up first; codec-leg exception documented |
| [ADR-012](ADR-012-privacy-posture-no-consumer-data-collected.md) | No telemetry / consumer data | Accepted | 2026-08-23 | No phone-home; model auto-download is the one exception |
| [ADR-013](ADR-013-posture-fully-open.md) | Posture: fully-open, private doc routing | Accepted | 2026-08-23 | No moat; planning docs stay in the private AI workspace |
| [ADR-014](ADR-014-layered-sync-core-with-port-traits.md) | Layered sync core, port traits | Accepted | 2026-08-23 | StorageEngine port; vector wiring concrete today (recorded gap) |

## ADR Template

```markdown
# ADR-XXX: Title

## Status
Proposed | Accepted | Deprecated | Superseded by ADR-YYY

## Date
YYYY-MM-DD

## Context
What is the issue that we're seeing that is motivating this decision?

## Decision
What is the change that we're proposing/doing?

## Consequences
What becomes easier or harder because of this change?

## References
Links to related code, docs, and tickets.
```

## Conventions

- ADR files are named `ADR-NNN-short-title.md`
- Numbers are sequential and never reused
- Superseded ADRs are kept for historical context (status updated)
- Each ADR should reference the relevant code paths and documentation
