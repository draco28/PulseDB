//! Integration tests for the in-process watch system (E4-S01).
//!
//! Tests the full stack: PulseDB facade → WatchService → crossbeam channel → WatchStream.
//! Uses External embedding provider (default), so all experiences must provide
//! pre-computed embeddings of the correct dimension (384 for D384).

use futures::executor::block_on;
use futures::StreamExt;

use pulsedb::{
    CollectiveId, Config, ExperienceUpdate, NewExperience, PulseDB, WatchEventType, WatchFilter,
};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Creates a dummy embedding of the correct dimension.
fn dummy_embedding() -> Vec<f32> {
    vec![0.1; DIM]
}

/// Helper to open a fresh database with default config.
fn open_db() -> (PulseDB, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = PulseDB::open(&path, Config::default()).unwrap();
    (db, dir)
}

/// Helper: open DB, create a collective, return both.
fn open_db_with_collective() -> (PulseDB, CollectiveId, tempfile::TempDir) {
    let (db, dir) = open_db();
    let cid = db.create_collective("test-collective").unwrap();
    (db, cid, dir)
}

/// Helper to build a minimal valid NewExperience for a given collective.
fn minimal_experience(collective_id: CollectiveId) -> NewExperience {
    NewExperience {
        collective_id,
        content: "Always validate user input before processing".to_string(),
        embedding: Some(dummy_embedding()),
        ..Default::default()
    }
}

// ============================================================================
// Core Event Delivery
// ============================================================================

#[test]
fn test_record_experience_emits_created_event() {
    let (db, cid, _dir) = open_db_with_collective();
    let mut stream = db.watch_experiences(cid);

    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    // Event should be available immediately (in-process, sync delivery)
    let event = block_on(stream.next()).expect("should receive event");
    assert_eq!(event.event_type, WatchEventType::Created);
    assert_eq!(event.experience_id, exp_id);
    assert_eq!(event.collective_id, cid);
}

#[test]
fn test_update_experience_emits_updated_event() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    // Subscribe AFTER recording so we only get the update event
    let mut stream = db.watch_experiences(cid);

    db.update_experience(
        exp_id,
        ExperienceUpdate {
            importance: Some(0.9),
            ..Default::default()
        },
    )
    .unwrap();

    let event = block_on(stream.next()).expect("should receive event");
    assert_eq!(event.event_type, WatchEventType::Updated);
    assert_eq!(event.experience_id, exp_id);
}

#[test]
fn test_archive_experience_emits_archived_event() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    let mut stream = db.watch_experiences(cid);
    db.archive_experience(exp_id).unwrap();

    let event = block_on(stream.next()).expect("should receive event");
    assert_eq!(event.event_type, WatchEventType::Archived);
    assert_eq!(event.experience_id, exp_id);
}

#[test]
fn test_unarchive_experience_emits_updated_event() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();
    db.archive_experience(exp_id).unwrap();

    let mut stream = db.watch_experiences(cid);
    db.unarchive_experience(exp_id).unwrap();

    let event = block_on(stream.next()).expect("should receive event");
    assert_eq!(event.event_type, WatchEventType::Updated);
    assert_eq!(event.experience_id, exp_id);
}

#[test]
fn test_delete_experience_emits_deleted_event() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    let mut stream = db.watch_experiences(cid);
    db.delete_experience(exp_id).unwrap();

    let event = block_on(stream.next()).expect("should receive event");
    assert_eq!(event.event_type, WatchEventType::Deleted);
    assert_eq!(event.experience_id, exp_id);
}

#[test]
fn test_reinforce_experience_emits_updated_event() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    let mut stream = db.watch_experiences(cid);
    let count = db.reinforce_experience(exp_id).unwrap();
    assert_eq!(count, 1);

    let event = block_on(stream.next()).expect("should receive event");
    assert_eq!(event.event_type, WatchEventType::Updated);
    assert_eq!(event.experience_id, exp_id);
}

// ============================================================================
// Multiple Subscribers
// ============================================================================

#[test]
fn test_multiple_subscribers_all_receive_events() {
    let (db, cid, _dir) = open_db_with_collective();
    let mut stream1 = db.watch_experiences(cid);
    let mut stream2 = db.watch_experiences(cid);

    let exp_id = db.record_experience(minimal_experience(cid)).unwrap();

    let event1 = block_on(stream1.next()).expect("stream1 should receive");
    let event2 = block_on(stream2.next()).expect("stream2 should receive");

    assert_eq!(event1.experience_id, exp_id);
    assert_eq!(event2.experience_id, exp_id);
    assert_eq!(event1.event_type, WatchEventType::Created);
    assert_eq!(event2.event_type, WatchEventType::Created);
}

// ============================================================================
// Filtering
// ============================================================================

#[test]
fn test_filtered_watch_by_domain() {
    let (db, cid, _dir) = open_db_with_collective();

    let filter = WatchFilter {
        domains: Some(vec!["security".to_string()]),
        ..Default::default()
    };
    let mut stream = db.watch_experiences_filtered(cid, filter);

    // Record experience WITH matching domain
    let matching = db
        .record_experience(NewExperience {
            collective_id: cid,
            content: "Always sanitize inputs".to_string(),
            embedding: Some(dummy_embedding()),
            domain: vec!["security".to_string()],
            ..Default::default()
        })
        .unwrap();

    // Record experience WITHOUT matching domain
    let _non_matching = db
        .record_experience(NewExperience {
            collective_id: cid,
            content: "Optimize hot loops".to_string(),
            embedding: Some(dummy_embedding()),
            domain: vec!["performance".to_string()],
            ..Default::default()
        })
        .unwrap();

    // Should only receive the matching event
    let event = block_on(stream.next()).expect("should receive matching event");
    assert_eq!(event.experience_id, matching);

    // No more events should be pending (non-matching was filtered)
    // We can't easily test "nothing more" with block_on without timeout,
    // but we can check the underlying channel is empty via Debug output.
}

#[test]
fn test_filtered_watch_by_importance() {
    let (db, cid, _dir) = open_db_with_collective();

    let filter = WatchFilter {
        min_importance: Some(0.7),
        ..Default::default()
    };
    let mut stream = db.watch_experiences_filtered(cid, filter);

    // Low importance — should be filtered
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Minor observation".to_string(),
        embedding: Some(dummy_embedding()),
        importance: 0.3,
        ..Default::default()
    })
    .unwrap();

    // High importance — should be delivered
    let important = db
        .record_experience(NewExperience {
            collective_id: cid,
            content: "Critical security vulnerability found".to_string(),
            embedding: Some(dummy_embedding()),
            importance: 0.9,
            ..Default::default()
        })
        .unwrap();

    let event = block_on(stream.next()).expect("should receive high-importance event");
    assert_eq!(event.experience_id, important);
}

// ============================================================================
// Collective Isolation
// ============================================================================

#[test]
fn test_watch_isolates_collectives() {
    let (db, _dir) = open_db();
    let cid_a = db.create_collective("collective-a").unwrap();
    let cid_b = db.create_collective("collective-b").unwrap();

    let mut stream_a = db.watch_experiences(cid_a);

    // Record in collective B
    db.record_experience(NewExperience {
        collective_id: cid_b,
        content: "Event in B".to_string(),
        embedding: Some(dummy_embedding()),
        ..Default::default()
    })
    .unwrap();

    // Record in collective A
    let exp_a = db
        .record_experience(NewExperience {
            collective_id: cid_a,
            content: "Event in A".to_string(),
            embedding: Some(dummy_embedding()),
            ..Default::default()
        })
        .unwrap();

    // Stream A should only see the event from collective A
    let event = block_on(stream_a.next()).expect("should receive event from A");
    assert_eq!(event.experience_id, exp_a);
    assert_eq!(event.collective_id, cid_a);
}

// ============================================================================
// Stream Lifecycle
// ============================================================================

#[test]
fn test_stream_ends_when_db_dropped() {
    let (db, cid, _dir) = open_db_with_collective();
    let mut stream = db.watch_experiences(cid);

    // Drop the database (closes it, drops all senders)
    db.close().unwrap();

    // Stream should return None (end of stream)
    let result = block_on(stream.next());
    assert!(result.is_none(), "stream should end after DB close");
}

#[test]
fn test_subscriber_removed_on_stream_drop() {
    let (db, cid, _dir) = open_db_with_collective();

    {
        let _stream = db.watch_experiences(cid);
        // stream exists — subscriber registered
    }
    // stream dropped — subscriber should be cleaned up

    // Record an experience to verify no stale subscribers cause issues
    let result = db.record_experience(minimal_experience(cid));
    assert!(result.is_ok(), "should not error with no subscribers");
}

// ============================================================================
// Buffer Behavior
// ============================================================================

#[test]
fn test_buffer_full_graceful_degradation() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let config = Config {
        watch: pulsedb::WatchConfig {
            buffer_size: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let db = PulseDB::open(&path, config).unwrap();
    let cid = db.create_collective("test").unwrap();

    let mut stream = db.watch_experiences(cid);

    // Fill the buffer with 2 events
    db.record_experience(minimal_experience(cid)).unwrap();
    db.record_experience(minimal_experience(cid)).unwrap();

    // Third event should be dropped (buffer full), but record_experience should NOT fail
    let result = db.record_experience(minimal_experience(cid));
    assert!(
        result.is_ok(),
        "record_experience should not fail when watch buffer is full"
    );

    // Only 2 events should be in the stream
    let e1 = block_on(stream.next());
    let e2 = block_on(stream.next());
    assert!(e1.is_some());
    assert!(e2.is_some());
}

// ============================================================================
// Event Ordering
// ============================================================================

#[test]
fn test_events_arrive_in_order() {
    let (db, cid, _dir) = open_db_with_collective();
    let mut stream = db.watch_experiences(cid);

    // Record 3 experiences
    let id1 = db.record_experience(minimal_experience(cid)).unwrap();
    let id2 = db.record_experience(minimal_experience(cid)).unwrap();
    let id3 = db.record_experience(minimal_experience(cid)).unwrap();

    // Events should arrive in order
    let e1 = block_on(stream.next()).unwrap();
    let e2 = block_on(stream.next()).unwrap();
    let e3 = block_on(stream.next()).unwrap();

    assert_eq!(e1.experience_id, id1);
    assert_eq!(e2.experience_id, id2);
    assert_eq!(e3.experience_id, id3);
}
