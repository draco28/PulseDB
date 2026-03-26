//! Integration tests for cross-process watch system (E4-S02).
//!
//! Tests the WAL sequence tracking and polling API through the PulseDB
//! facade. While true cross-process testing would require spawning
//! separate processes, these tests validate the core mechanism:
//! sequence tracking, event recording, and polling.

use pulsedb::{CollectiveId, Config, ExperienceUpdate, NewExperience, PulseDB, WatchEventType};
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

    // Collective creation is WAL event #1
    db.record_experience(minimal_experience(cid)).unwrap();
    assert_eq!(db.get_current_sequence().unwrap(), 2);

    db.record_experience(minimal_experience(cid)).unwrap();
    assert_eq!(db.get_current_sequence().unwrap(), 3);

    db.record_experience(minimal_experience(cid)).unwrap();
    assert_eq!(db.get_current_sequence().unwrap(), 4);
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

    // Collective creation is WAL event #1; experience is #2
    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 1); // Only experience events (filtered)
    assert_eq!(seq, 2); // Seq includes collective event

    let event = &events[0];
    assert_eq!(event.experience_id, exp_id);
    assert_eq!(event.collective_id, cid);
    assert_eq!(event.event_type, WatchEventType::Created);
}

#[test]
fn test_poll_changes_incremental() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 3 experiences (collective = seq 1, exps = seq 2,3,4)
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();

    // First poll: get 3 experience events (filtered from 4 total)
    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 4);

    // Record 2 more (seq 5,6)
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();

    // Second poll: only 2 new experience events
    let (events, seq) = db.poll_changes(4).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(seq, 6);

    // Third poll: no new events
    let (events, seq) = db.poll_changes(6).unwrap();
    assert_eq!(events.len(), 0);
    assert_eq!(seq, 6);
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

    // Collective(1) + 6 experience events = 7 total, 6 experience-only
    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 6);
    assert_eq!(seq, 7);

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

    // Collective = seq 1, 10 experiences = seq 2-11. Total 11 WAL events.
    // poll_changes_batch filters to experience-only but batch_limit applies
    // to the storage-level poll (which includes all entity types).
    // Batch limit 3 gets WAL events 1-3 (collective + 2 experiences) → 2 exp events
    let (events, seq) = db.poll_changes_batch(0, 3).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(seq, 3);

    // Continue from seq 3: gets events 4-6 (3 experiences) → 3 exp events
    let (events, seq) = db.poll_changes_batch(3, 3).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 6);

    // Continue: events 7-9 (3 experiences) → 3 exp events
    let (events, seq) = db.poll_changes_batch(6, 3).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 9);

    // Get remaining: events 10-11 (2 experiences) → 2 exp events
    let (events, seq) = db.poll_changes_batch(9, 100).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(seq, 11);
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

        // Collective(1) + 3 experiences(2,3,4) = seq 4
        assert_eq!(db.get_current_sequence().unwrap(), 4);
        db.close().unwrap();
    }

    // Reopen and verify
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();

        // Sequence should persist
        assert_eq!(db.get_current_sequence().unwrap(), 4);

        // Events should be retrievable (3 experience events, filtered)
        let (events, seq) = db.poll_changes(0).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(seq, 4);
        assert!(events
            .iter()
            .all(|e| e.event_type == WatchEventType::Created));

        // New writes continue from 4
        db.record_experience(minimal_experience(cid)).unwrap();
        assert_eq!(db.get_current_sequence().unwrap(), 5);

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

    // poll_changes returns experience events (cross-collective) — the WAL is global
    // 2 collectives (seq 1,2) + 3 experiences (seq 3,4,5) → 3 experience events
    let (events, seq) = db.poll_changes(0).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(seq, 5);

    // But each event has the correct collective_id
    assert_eq!(events[0].collective_id, cid_a);
    assert_eq!(events[1].collective_id, cid_b);
    assert_eq!(events[2].collective_id, cid_a);
}
