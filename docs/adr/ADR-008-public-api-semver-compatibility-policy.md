# ADR-008: Public API SemVer Compatibility Policy

## Status
Accepted

## Date
2026-08-23

## Context
PulseDB is published as a library crate on crates.io and must provide stable API guarantees to consumers. Breaking changes require minor version bumps per SemVer.

## Decision
PulseDB follows Semantic Versioning. Public API changes that break existing code (removed methods, changed signatures) bump the MINOR version. Pre-1.0, backward-compatible **additions also bump MINOR** (matching `docs/09-Developer-Guide.md` and the actual release history: features → MINOR, fixes → PATCH); after 1.0, additions keep bumping MINOR per SemVer and PATCH stays reserved for fixes. (Struct field reordering is not breaking in Rust and was removed from this list.) The public API is everything `src/lib.rs` re-exports or exposes (including the `embedding`, `storage`, `substrate`, `vector`, feature-gated `sync`, and the `config`/`collective`/`experience`/`relation`/`insight`/`activity`/`search`/`watch` item re-exports), plus `src/types.rs` and `src/error.rs`. The touch surface names the primary contract files, not every contributing module — a maintainer judging a bump should check the exported items, not just these paths.

**MSRV.** The minimum supported Rust version (`rust-version` in `Cargo.toml`) is part of the compatibility contract but is not an API break: an MSRV bump lands in a MINOR release, is recorded in the CHANGELOG, and is enforced by the version-agnostic `MSRV` CI job (whose pinned toolchain must match `Cargo.toml`). 2026-09: 1.89 → 1.90 (redb 4.2 requires rustc 1.90).

## Touch surface
`src/lib.rs`, `src/types.rs`, `src/error.rs`, `src/substrate/**`

## Revisit trigger
Revisit when breaking changes are needed for a new minor version.
