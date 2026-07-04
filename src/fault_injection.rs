//! Test-only migration fault-injection seam (VS-4.0.4 work 4.05, issue #46).
//!
//! Compiled **only** under `--features fault-injection` — never in default or
//! release builds, so the production migration path is byte-identical when the
//! feature is off (each call site is a statement-level `#[cfg(...)]` that
//! disappears entirely). This lets a test ARM a simulated crash at a specific
//! migration boundary; the migration path calls [`maybe_inject`] at each of the
//! five boundaries, which either `panic!`s (in-process — Drop runs, so the redb
//! write-txn aborts gracefully / MVCC rolls back) or `raise(SIGKILL)`s (the one
//! subprocess crash-fidelity test — no Drop, forcing redb's file-level recovery).
//!
//! The armed state is **thread-local**: the migration runs synchronously on the
//! same thread that calls `PulseDB::open`, so arming on that thread is sufficient
//! and there is zero cross-test contamination (each `#[test]` gets its own
//! thread, hence its own register).
//!
//! This module changes NO migration behavior — it only observes boundaries. The
//! boundary ordering/semantics are owned by 4.03 (registry re-encode loop) and
//! 4.04 (reordered pre-txn path + backup/fsync + marker rules).

use std::cell::Cell;

/// The five migration boundaries a test can crash at.
///
/// Two are **pre-txn** (a crash there is NOT covered by a redb txn abort):
/// - [`Boundary::MidBackupPreFsync`] — inside `backup_once`, after the sidecar
///   bytes are copied but before the `#53c` fsync makes them durable.
/// - [`Boundary::PostRedbUpgrade`] — after the destructive in-place redb v2→v3
///   upgrade returns, before the redb-4.1 reopen.
///
/// Three are **write-txn** (a crash before `commit()` rolls the whole txn back):
/// - [`Boundary::PreReencode`] — before the registry-driven codec re-encode pass.
/// - [`Boundary::MidReencode`] — inside the re-encode loop, after ≥1 table pass.
/// - [`Boundary::PreMarker`] — after re-encode, just before the marker insert
///   (the atomic commit point).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Boundary {
    /// Pre-txn: inside `backup_once`, after the sidecar bytes are copied but
    /// before the `#53c` fsync makes them durable.
    MidBackupPreFsync,
    /// Pre-txn: after the destructive in-place redb v2→v3 upgrade returns, before
    /// the redb-4.1 reopen.
    PostRedbUpgrade,
    /// Write-txn: before the registry-driven codec re-encode pass begins.
    PreReencode,
    /// Write-txn: partway through the re-encode loop, after ≥1 table pass.
    MidReencode,
    /// Write-txn: after the re-encode succeeded, just before the marker insert
    /// (the atomic commit point).
    PreMarker,
}

/// How to crash when the armed boundary is reached.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Unwind via `panic!` — Drop runs (graceful redb txn abort). Deterministic,
    /// CI-stable, catchable with `catch_unwind`; covers every boundary.
    Panic,
    /// `libc::raise(SIGKILL)` — the process dies immediately, no Drop. Used by
    /// exactly one subprocess test at `PreMarker` for genuine crash fidelity.
    Sigkill,
}

thread_local! {
    static ARMED: Cell<Option<(Boundary, Action)>> = const { Cell::new(None) };
}

/// Arm a crash at `boundary` with `action`, on the current thread.
pub fn arm(boundary: Boundary, action: Action) {
    ARMED.with(|c| c.set(Some((boundary, action))));
}

/// Clear any armed injection on the current thread.
pub fn disarm() {
    ARMED.with(|c| c.set(None));
}

/// RAII: arm on construction, disarm on drop — so a panicking (in-process)
/// injection can't leave the register armed for a later assertion on the same
/// thread. Cheap; tests may also call [`disarm`] explicitly.
pub struct ArmGuard;

impl ArmGuard {
    /// Arm the given `boundary`/`action` on the current thread; the returned
    /// guard disarms on drop.
    pub fn new(boundary: Boundary, action: Action) -> Self {
        arm(boundary, action);
        ArmGuard
    }
}

impl Drop for ArmGuard {
    fn drop(&mut self) {
        disarm();
    }
}

/// Called by the migration path at each boundary. When the armed boundary
/// matches, crash per the armed action; otherwise a no-op.
pub(crate) fn maybe_inject(boundary: Boundary) {
    let armed = ARMED.with(|c| c.get());
    if let Some((b, action)) = armed {
        if b == boundary {
            // Consume the arm so a retry/loop can't re-trigger on the same thread.
            disarm();
            match action {
                Action::Panic => {
                    panic!("fault-injection: simulated migration crash at {boundary:?}")
                }
                Action::Sigkill => {
                    // SAFETY: `raise` with a valid signal number is always sound;
                    // SIGKILL terminates this process immediately (no unwinding,
                    // no Drop) — the point of the crash-fidelity test.
                    unsafe {
                        libc::raise(libc::SIGKILL);
                    }
                    // Unreachable in practice; guard against a spurious return.
                    unreachable!("SIGKILL did not terminate the process");
                }
            }
        }
    }
}
