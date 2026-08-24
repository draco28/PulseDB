# ADR-008: Public API SemVer Compatibility Policy

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is published as a library crate on crates.io and must provide stable API guarantees to consumers. Breaking changes require minor version bumps per SemVer.

## Decision
PulseDB follows Semantic Versioning. Public API changes that break existing code (removed methods, changed signatures, struct field reordering) bump the MINOR version. Additions are PATCH **as a pre-1.0 convention only** — after 1.0, backward-compatible additions bump MINOR per SemVer and PATCH is reserved for fixes. The public API is everything `src/lib.rs` re-exports or exposes (including the `embedding`, `storage`, `substrate`, `vector`, feature-gated `sync`, and the `config`/`collective`/`experience`/`relation`/`insight`/`activity`/`search`/`watch` item re-exports), plus `src/types.rs` and `src/error.rs`. The touch surface names the primary contract files, not every contributing module — a maintainer judging a bump should check the exported items, not just these paths.

## Touch surface
`src/lib.rs`, `src/types.rs`, `src/error.rs`, `src/substrate/**`

## Revisit trigger
Revisit when breaking changes are needed for a new minor version.
