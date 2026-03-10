//! Integration tests for cross-process watch system (E4-S02).
//!
//! Tests the WAL sequence tracking and polling API through the PulseDB
//! facade. While true cross-process testing would require spawning
//! separate processes, these tests validate the core mechanism:
//! sequence tracking, event recording, and polling.

use pulsedb::{
    CollectiveId, Config, ExperienceUpdate, NewExperience, PulseDB, WatchEventType,
};
use tempfile::tempdir;

/// Default embedding dimension for tests.
const DIM: usize = 384;

fn dummy_embedding() -> Vec<f32> {
    vec![0.1; DIM]
}

fn open_db() -> (PulseDB, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = PulseDB::open(&path, Config::default()).unwrap();
    (db, dir)
}

fn open_db_with_collective() -> (PulseDB, CollectiveId, tempfile::TempDir) {
    let (db, dir) = open_db();
    let cid = db.create_collective("test-collective").unwrap();
    (db, cid, dir)
}

fn minimal_experience(collective_id: CollectiveId) -> NewExperience {
    NewExperience {
        collective_id,
        content: "Cross-process watch test experience".to_string(),
        embedding: Some(dummy_embedding()),
        ..Default::default()
    }
}

// ============================================================================
// get_current_sequence tests
// ============================================================================

#[test]
fn test_get_current_sequence_starts_at_zero() {
    let (db, _dir) = open_db();
    assert_eq!(db.get_current_sequence().unwrap(), 0);
}

#[test]
fn test_get_current_sequence_returns_latest() {
    let (db, cid, _dir) = open_db_with_collective();

    db.record_experience(minimal_experience(cid)).unwrap();
    assert_eq!(db.get_current_sequence().unwrap(), 1);

    db.record_experience(minimal_experience(cid)).unwrap();
    assert_eq!(db.get_current_sequence().unwrap(), 2);

    db.record_experience(minimal_experience(cid)).unwrap();
    assert_eq!(db.get_current_sequence().unwrap(), 3);
}

// ============================================================================
// poll_changes tests
// ============================================================================

#[test]
fn test_poll_changes_empty_database() {
    let (db, _dir) = open_db();

    let (events, seq) = db.poll_changes(0).unwrap();
    assert!(events.is_empty());
    assert_eq!(seq, 0);
}

#[test]
fn test_poll_changes_after_record() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(seq, 1);

    let event = &events[0];
    assert_eq!(event.experience_id, exp_id);
    assert_eq!(event.collective_id, cid);
    assert_eq!(event.event_type, WatchEventType::Created);
}

#[test]
fn test_poll_changes_incremental() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 3 experiences
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();

    // First poll: get all 3
    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 3);

    // Record 2 more
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();

    // Second poll: only 2 new events
    let (events, seq) = db.poll_changes(3).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(seq, 5);

    // Third poll: no new events
    let (events, seq) = db.poll_changes(5).unwrap();
    assert_eq!(events.len(), 0);
    assert_eq!(seq, 5);
}

#[test]
fn test_poll_changes_mixed_operations() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record
    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    // Update
    db.update_experience(
        exp_id,
        ExperienceUpdate {
            importance: Some(0.99),
            ..Default::default()
        },
    )
    .unwrap();

    // Reinforce
    db.reinforce_experience(exp_id).unwrap();

    // Archive
    db.update_experience(
        exp_id,
        ExperienceUpdate {
            archived: Some(true),
            ..Default::default()
        },
    )
    .unwrap();

    // Unarchive
    db.update_experience(
        exp_id,
        ExperienceUpdate {
            archived: Some(false),
            ..Default::default()
        },
    )
    .unwrap();

    // Delete
    db.delete_experience(exp_id).unwrap();

    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 6);
    assert_eq!(seq, 6);

    assert_eq!(events[0].event_type, WatchEventType::Created);
    assert_eq!(events[1].event_type, WatchEventType::Updated); // importance change
    assert_eq!(events[2].event_type, WatchEventType::Updated); // reinforce
    assert_eq!(events[3].event_type, WatchEventType::Archived); // archive
    assert_eq!(events[4].event_type, WatchEventType::Updated); // unarchive
    assert_eq!(events[5].event_type, WatchEventType::Deleted);
}

#[test]
fn test_poll_changes_batch_limit() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 10 experiences
    for _ in 0..10 {
        db.record_experience(minimal_experience(cid)).unwrap();
    }

    // Poll with batch limit of 3
    let (events, seq) = db.poll_changes_batch(0, 3).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 3);

    // Continue
    let (events, seq) = db.poll_changes_batch(3, 3).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 6);

    // Get remaining
    let (events, seq) = db.poll_changes_batch(6, 100).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(seq, 10);
}

#[test]
fn test_sequence_survives_close_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    // Open, create collective, record experiences
    let cid;
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();
        cid = db.create_collective("test").unwrap();
        db.record_experience(minimal_experience(cid)).unwrap();
        db.record_experience(minimal_experience(cid)).unwrap();
        db.record_experience(minimal_experience(cid)).unwrap();

        assert_eq!(db.get_current_sequence().unwrap(), 3);
        db.close().unwrap();
    }

    // Reopen and verify
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();

        // Sequence should persist
        assert_eq!(db.get_current_sequence().unwrap(), 3);

        // Events should be retrievable
        let (events, seq) = db.poll_changes(0).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(seq, 3);
        assert!(events.iter().all(|e| e.event_type == WatchEventType::Created));

        // New writes continue from 3
        db.record_experience(minimal_experience(cid)).unwrap();
        assert_eq!(db.get_current_sequence().unwrap(), 4);

        db.close().unwrap();
    }
}

#[test]
fn test_poll_changes_collective_isolation() {
    let (db, _dir) = open_db();

    let cid_a = db.create_collective("alpha").unwrap();
    let cid_b = db.create_collective("beta").unwrap();

    // Record in both collectives
    db.record_experience(minimal_experience(cid_a)).unwrap();
    db.record_experience(minimal_experience(cid_b)).unwrap();
    db.record_experience(minimal_experience(cid_a)).unwrap();

    // poll_changes returns ALL events (cross-collective) — the WAL is global
    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 3);

    // But each event has the correct collective_id
    assert_eq!(events[0].collective_id, cid_a);
    assert_eq!(events[1].collective_id, cid_b);
    assert_eq!(events[2].collective_id, cid_a);
}
