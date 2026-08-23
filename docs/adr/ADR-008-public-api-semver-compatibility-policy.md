# ADR-008: Public API SemVer Compatibility Policy

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is published as a library crate on crates.io and must provide stable API guarantees to consumers. Breaking changes require minor version bumps per SemVer.

## Decision
PulseDB follows Semantic Versioning. Public API changes that break existing code (removed methods, changed signatures, struct field reordering) bump the MINOR version. Additions are PATCH. The public API consists of `src/lib.rs`, `src/types/`, and `src/error.rs`. Internal modules may change without bumping.

## Touch surface
`src/lib.rs`, `src/types/`, `src/error.rs`, `CLAUDE.md`, `README.md`

## Revisit trigger
Revisit when breaking changes are needed for a new minor version.
