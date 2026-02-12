//! ACID and crash recovery integration tests for PulseDB.
//!
//! These tests verify that the storage layer provides the expected
//! durability and atomicity guarantees at the PulseDB level.
//!
//! # Crash Simulation
//!
//! We simulate a crash by dropping the `PulseDB` handle without calling
//! `close()`. Since redb durably commits data during `commit()` (not during
//! `close()`), dropping the handle simulates an ungraceful shutdown.
//!
//! redb uses shadow paging (not a WAL), so the database is always in a
//! consistent state: either the commit completed (data is present) or it
//! didn't (data is absent). There is never a half-committed state.

use pulsedb::{Config, PulseDB};
use tempfile::tempdir;

/// Helper: open a PulseDB at the given path with default config.
fn open_db(path: &std::path::Path) -> PulseDB {
    PulseDB::open(path, Config::default()).unwrap()
}

// ============================================================================
// Durability Tests
// ============================================================================

#[test]
fn test_committed_data_survives_normal_close() {
    // Basic durability: save data, close gracefully, reopen, verify.
    let dir = tempdir().unwrap();
    let path = dir.path().join("durable.db");

    // Write and close normally
    let db = open_db(&path);
    let storage = db.storage_for_test();
    let collective = pulsedb::Collective::new("durable-project", 384);
    let id = collective.id;
    storage.save_collective(&collective).unwrap();
    db.close().unwrap();

    // Reopen and verify
    let db = open_db(&path);
    let storage = db.storage_for_test();
    let retrieved = storage.get_collective(id).unwrap();
    assert!(retrieved.is_some(), "Data must survive a normal close");
    assert_eq!(retrieved.unwrap().name, "durable-project");
    db.close().unwrap();
}

#[test]
fn test_committed_data_survives_crash() {
    // Crash durability: save data, DROP without close (simulates crash),
    // reopen, verify data is present.
    let dir = tempdir().unwrap();
    let path = dir.path().join("crash.db");

    let collective_id;
    {
        let db = open_db(&path);
        let storage = db.storage_for_test();
        let collective = pulsedb::Collective::new("crash-safe", 384);
        collective_id = collective.id;
        storage.save_collective(&collective).unwrap();
        // NO close() -- simulates crash (drop without flush)
    }

    // Reopen and verify
    let db = open_db(&path);
    let storage = db.storage_for_test();
    let retrieved = storage.get_collective(collective_id).unwrap();
    assert!(
        retrieved.is_some(),
        "Committed data must survive a crash (drop without close)"
    );
    assert_eq!(retrieved.unwrap().name, "crash-safe");
    db.close().unwrap();
}

#[test]
fn test_bulk_data_survives_crash() {
    // Crash durability at scale: write 100 collectives, crash, verify
    // all 100 are present after recovery.
    let dir = tempdir().unwrap();
    let path = dir.path().join("bulk_crash.db");

    let mut ids = Vec::new();
    {
        let db = open_db(&path);
        let storage = db.storage_for_test();
        for i in 0..100 {
            let collective = pulsedb::Collective::new(format!("project-{}", i), 384);
            ids.push(collective.id);
            storage.save_collective(&collective).unwrap();
        }
        // NO close() -- crash
    }

    // Reopen and verify all 100
    let db = open_db(&path);
    let storage = db.storage_for_test();
    let collectives = storage.list_collectives().unwrap();
    assert_eq!(
        collectives.len(),
        100,
        "All 100 collectives must survive crash"
    );

    // Verify each ID is present
    for id in &ids {
        assert!(
            storage.get_collective(*id).unwrap().is_some(),
            "Collective {} must be present after crash",
            id
        );
    }
    db.close().unwrap();
}

#[test]
fn test_multiple_crash_cycles() {
    // Multiple crash/recovery cycles should not cause corruption.
    let dir = tempdir().unwrap();
    let path = dir.path().join("multi_crash.db");

    // Cycle 1: create and crash
    let id1;
    {
        let db = open_db(&path);
        let storage = db.storage_for_test();
        let c = pulsedb::Collective::new("cycle-1", 384);
        id1 = c.id;
        storage.save_collective(&c).unwrap();
    }

    // Cycle 2: add more and crash again
    let id2;
    {
        let db = open_db(&path);
        let storage = db.storage_for_test();

        // Verify cycle 1 data survived
        assert!(storage.get_collective(id1).unwrap().is_some());

        let c = pulsedb::Collective::new("cycle-2", 384);
        id2 = c.id;
        storage.save_collective(&c).unwrap();
    }

    // Cycle 3: verify both survived
    let db = open_db(&path);
    let storage = db.storage_for_test();
    assert!(storage.get_collective(id1).unwrap().is_some());
    assert!(storage.get_collective(id2).unwrap().is_some());
    assert_eq!(storage.list_collectives().unwrap().len(), 2);
    db.close().unwrap();
}
