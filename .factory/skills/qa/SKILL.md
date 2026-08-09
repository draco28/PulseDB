---
name: qa
description: >
  Run functional QA tests for PulseDB — an embedded Rust database library.
  Analyzes the git diff to determine which API areas changed, writes and runs
  small Rust integration programs that exercise the public API as a real
  consumer would (open → create → record → search → reopen → verify), across
  all 5 feature configurations. Use when testing PRs, releases, or smoke
  testing the library's public API surface.
---

# QA Orchestrator — PulseDB (Rust library crate)

**SCOPE: This skill performs functional/consumer-simulation QA — writing small Rust
programs that exercise the public API as a real consumer would, then compiling and running
them to verify correct behavior. Do NOT report on CI checks, linting, clippy, fmt, or
unit tests already in the suite. Those are handled by the existing ci.yml workflow.**

**This is a LIBRARY crate, not a web/CLI/API application.** There is no browser, no TUI,
no HTTP endpoint. The "app" is the `pulsedb` crate consumed in-process. Functional QA
means: does the public API work as documented for a real downstream consumer?

## Step 1: Load Configuration

Read `.factory/skills/qa/config.yaml` for features, personas, and app definitions.

## Step 2: Determine Target

The target is always `local` — PulseDB is an embedded library running in-process. There
are no remote environments. All tests run via `cargo test` in the checked-out branch.

**CRITICAL: Always test against the checked-out PR branch code.** Do NOT test against
a published crates.io version or a different checkout. The QA program must `use pulsedb::*`
from the workspace root.

## Step 3: Analyze Git Diff

Run `git diff origin/main...HEAD` to determine what changed. Map changed files to API
areas using the path_patterns in config.yaml. The `library` app covers ALL of `src/**`,
`Cargo.toml`, and `tests/**`.

API-area routing (used to select which flows to run from the sub-skill):

| Changed path | API area | Relevant flows |
|---|---|---|
| `src/db.rs` | core lifecycle, embedding injection, cross-provider | lifecycle, embedding, cross-provider |
| `src/embedding/**` | embedding identity, OnnxEmbedding | embedding, cross-provider |
| `src/storage/**` | storage engine, schema, migration | lifecycle, cross-provider |
| `src/search/**` | vector search, context | search |
| `src/sync/**` | sync replication | sync |
| `src/insight/**`, `src/relation/**` | insights, relations | insights |
| `src/experience/**` | experience records | lifecycle |
| `src/collective/**` | collectives | lifecycle |
| `src/substrate/**` | SubstrateProvider trait | substrate |
| `src/watch/**` | watch system | watch |
| `src/config.rs` | configuration | lifecycle |
| `src/error.rs` | error types | all (error variants affect every flow) |
| `Cargo.toml` | dependency changes | all (dep changes can break compilation) |

If NO `src/` or `Cargo.toml` changes (e.g., docs-only, CI-only, `.factory/`-only),
report as INCONCLUSIVE: "No library code changed — QA not applicable for this diff."

## Step 4: Pre-flight Checks

1. **Rust toolchain available:** `rustc --version` and `cargo --version`
2. **Workspace compiles:** `cargo build` (default features) — if this fails, report BLOCKED
3. **Feature-specific compile:** for each feature config that will be tested, run
   `cargo build <flag>` — if any fails to compile, report BLOCKED for that feature set

Do NOT run pre-flight for feature sets NOT affected by the diff.

## Step 5: Execute Consumer-Simulation Flows

Read `.factory/skills/qa-library/SKILL.md` for the full menu of test flows.

For each flow relevant to the diff:
1. Write a small Rust integration test to `tests/qa_flow_<name>.rs` that exercises the
   API as a real consumer would
2. Run it with `cargo test --test qa_flow_<name> <feature-flag> -- --nocapture`
3. Capture the output (test result + any println output)
4. Record PASS/FAIL/BLOCKED with evidence

**Consumer simulation principles:**
- Use ONLY the public API (`use pulsedb::*`) — never access internals
- Follow real-world usage patterns from the README's Quick Start
- Use `tempfile::TempDir` for isolation (auto-cleanup)
- Assert observable behavior, not internal state
- Include at least 1 negative test (error handling, boundary conditions)

## Step 6: Feature-Set Matrix

For flows affected by the diff, run them across ALL 5 feature configurations
(default, builtin-embeddings, sync, sync-http, sync-websocket) — UNLESS the flow
is clearly feature-independent (e.g., core lifecycle doesn't need sync).

Feature-applicability:
- **Core lifecycle, search, insights, decay:** default features sufficient
- **Embedding injection, OnnxEmbedding identity:** needs `--features builtin-embeddings`
- **Sync flows:** needs `--features sync` (or sync-http/sync-websocket for transport-specific)
- **Cross-provider prevention:** needs `--features builtin-embeddings` for OnnxEmbedding tests; default for External/stub tests

## Step 7: Evidence Capture

For each test step, capture:
- The Rust test code written (fenced code block)
- The cargo test output (fenced code block with test result line)
- Any println! output showing API behavior

Evidence quality rules:
- Show the RELEVANT output — trim verbose compilation noise
- Label each evidence block: what it shows and why it matters
- Include the feature flag used

## Step 8: Test Quality Gate

1. **CHANGE-SPECIFIC FIRST.** At least half your tests should directly verify the behavioral change in the diff.
2. **CONSUMER PERSPECTIVE.** All tests use the public API as a real consumer would — never internal types or private functions.
3. **NEGATIVE TESTS.** Include at least 1 test verifying error handling or boundary conditions.
4. **FEATURE MATRIX.** Test across feature configurations when the change is feature-gated.
5. **NO UNIT TEST DUPLICATION.** Do NOT re-run existing `#[test]` functions from `src/`. Write NEW consumer-simulation programs.
6. **INCONCLUSIVE IF UNSURE.** If you cannot articulate what the PR changes, mark as INCONCLUSIVE.

## Step 9: Handle Failures

Never silently skip a flow. If a flow cannot complete (compile error, panic, timeout),
report it as BLOCKED with what was tried and how to fix it. Continue with other flows.

## Step 10: Generate Report

Generate the report at `./qa-results/report.md` using `.factory/skills/qa/REPORT-TEMPLATE.md`.

## Step 11: Suggest Skill Updates (Failure Learning)

After generating the report, check if any BLOCKED or FAIL results revealed a testing
environment insight. Format as a "Suggested Skill Updates" table with severity, the
issue, and a ready-to-copy fix prompt. Only suggest genuine environment insights, not
expected behavior changes or skill bugs.

Clean up any `tests/qa_flow_*.rs` files you created after running them (they should not
be committed to the repo unless the user explicitly wants them).
