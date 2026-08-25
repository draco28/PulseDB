# ADR-013: Posture — fully-open with private doc routing

- **Status:** Accepted
- **Date:** 2026-08-23 (settled after two same-day reversals at the adoption critic-moment disposition; supersedes the open-core and fully-private drafts before either was committed)

## Context

PulseDB is public (crates.io `pulsehive-db`, public GitHub repo) under AGPL-3.0-only + commercial dual-license. The operator's settled position: **PulseDB is infrastructure, not a product line** — what downstream systems build on it is where productization happens. PulseDB therefore stays open source on its current license and keeps publishing to crates.io; the **planning and internal docs always stay private** in the AI-workspace repo.

## Decision

**Posture: fully-open.** All code open; **no functionality moat** — the moat inventory is deliberately empty. What is private is *doc routing*, which is artifact routing, not a moat: internal planning (specs-in-progress, sprint slices, the memory bank) lives in the private AI workspace; this ADR series, user-facing docs, and the public product-facing roadmap (`ROADMAP.md`, indexed from `docs/README.md`) are the public exception. Revenue intent `license` rides the existing AGPL + commercial dual-license; the license is the revenue mechanism, no private channel required.

Channel: **none** (inventory empty, C0). The gitignored `SPEC.md` pattern in the public working tree stays covered by `.gitignore` + the `PUBLIC_BOUNDARY.md` never-tracked rules — hygiene, not a moat.

## Consequences

- crates.io publishing continues on the existing tag-gated flow; nothing changes operationally.
- `PUBLIC_BOUNDARY.md` keeps machine-checkable never-tracked rules + the hygiene allowlist (patterns only, never content).
- The AI workspace remains the single private surface; any moat-intent later (private intelligence, private crate behind the ranking port) is a posture-supersede ceremony — one later ceremony, not a rewrite.

## Revisit trigger

Revisit when the first commercial license is negotiated, or if a revenue intent beyond the current dual-license appears.

### Verified claims

- Operator intent stated 2026-08-23 at the adoption review: keep public + crates.io, current license, planning docs private.
- License: AGPL-3.0-only + commercial (`LICENSING.md`, `Cargo.toml`).
- `SPEC.md` is covered by a `.gitignore` pattern plus the `never-tracked: **/SPEC.md` machine rule (presence in any given working tree is a local state, not a repo claim).

### Unverified claims

- None.
