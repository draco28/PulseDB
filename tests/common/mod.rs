//! Shared helpers for the integration-test binaries under `tests/`.
//!
//! Each `tests/*.rs` file is its own crate; a file opts in with `mod common;`.
//! Helpers that only some binaries use are expected to be dead code in the
//! others, hence the crate-level allow.

#![allow(dead_code)]

use std::path::PathBuf;

/// The committed golden-fixture directory (`tests/fixtures`).
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Copy the committed fixture to a fresh temp path and return the copy.
///
/// The on-open migration is destructive/in-place (redb's v2→v3 `upgrade()`
/// rewrites the file), so a test must never open the checked-in blob itself.
/// The returned `TempDir` owns the copy; keep it alive for as long as the
/// store is in use.
pub fn copy_fixture(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join(name);
    std::fs::copy(fixtures_dir().join(name), &dst).unwrap_or_else(|e| panic!("copy {name}: {e}"));
    (dir, dst)
}

/// One full bidirectional sync cycle between two stores over in-memory
/// transports (feature `sync`).
///
/// An `InMemorySyncTransport` pair is one direction, so A→B and B→A each get
/// their own pair, and each side pushes before the other pulls — the
/// sequencing the sync-engine G-counter convergence test established.
///
/// Managers are constructed *inside* this helper, i.e. after any
/// `remint_instance_id` the caller performed: `SyncManager` reads the store's
/// identity once, at construction.
#[cfg(feature = "sync")]
pub async fn sync_both_ways(
    db_a: &std::sync::Arc<pulsedb::PulseDB>,
    db_b: &std::sync::Arc<pulsedb::PulseDB>,
) {
    use pulsedb::sync::config::{SyncConfig, SyncDirection};
    use pulsedb::sync::manager::SyncManager;
    use pulsedb::sync::transport_mem::InMemorySyncTransport;
    use std::sync::Arc;

    let cfg = |direction| SyncConfig {
        direction,
        batch_size: 250,
        ..Default::default()
    };
    let (a_push, b_pull) = InMemorySyncTransport::new_pair();
    let (b_push, a_pull) = InMemorySyncTransport::new_pair();

    let mut mgr_a_push = SyncManager::new(
        Arc::clone(db_a),
        Box::new(a_push),
        cfg(SyncDirection::PushOnly),
    );
    let mut mgr_b_pull = SyncManager::new(
        Arc::clone(db_b),
        Box::new(b_pull),
        cfg(SyncDirection::PullOnly),
    );
    let mut mgr_b_push = SyncManager::new(
        Arc::clone(db_b),
        Box::new(b_push),
        cfg(SyncDirection::PushOnly),
    );
    let mut mgr_a_pull = SyncManager::new(
        Arc::clone(db_a),
        Box::new(a_pull),
        cfg(SyncDirection::PullOnly),
    );

    mgr_a_push.sync_once().await.unwrap();
    mgr_b_pull.sync_once().await.unwrap();
    mgr_b_push.sync_once().await.unwrap();
    mgr_a_pull.sync_once().await.unwrap();
}
