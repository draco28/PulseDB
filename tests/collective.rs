//! Integration tests for collective management operations (E1-S02).
//!
//! Tests the full stack: PulseDB facade → StorageEngine → redb.

use pulsedb::{CollectiveId, Config, EmbeddingDimension, PulseDB};
use tempfile::tempdir;

/// Helper to open a fresh database with default config.
fn open_db() -> (PulseDB, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = PulseDB::open(&path, Config::default()).unwrap();
    (db, dir)
}

// ============================================================================
// Create Collective
// ============================================================================

#[test]
fn test_create_collective() {
    let (db, _dir) = open_db();

    let id = db.create_collective("my-project").unwrap();

    let collective = db.get_collective(id).unwrap().unwrap();
    assert_eq!(collective.name, "my-project");
    assert_eq!(collective.embedding_dimension, 384); // default D384
    assert!(collective.owner_id.is_none());

    db.close().unwrap();
}

#[test]
fn test_create_collective_with_owner() {
    let (db, _dir) = open_db();

    let id = db
        .create_collective_with_owner("team-project", "user-42")
        .unwrap();

    let collective = db.get_collective(id).unwrap().unwrap();
    assert_eq!(collective.name, "team-project");
    assert_eq!(collective.owner_id.as_deref(), Some("user-42"));

    db.close().unwrap();
}

#[test]
fn test_create_collective_empty_name_rejected() {
    let (db, _dir) = open_db();

    let result = db.create_collective("");
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    db.close().unwrap();
}

#[test]
fn test_create_collective_whitespace_name_rejected() {
    let (db, _dir) = open_db();

    let result = db.create_collective("   ");
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    db.close().unwrap();
}

#[test]
fn test_create_collective_long_name_rejected() {
    let (db, _dir) = open_db();

    let long_name = "x".repeat(256);
    let result = db.create_collective(&long_name);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    db.close().unwrap();
}

#[test]
fn test_create_collective_max_length_name_accepted() {
    let (db, _dir) = open_db();

    let name = "x".repeat(255);
    let id = db.create_collective(&name).unwrap();

    let collective = db.get_collective(id).unwrap().unwrap();
    assert_eq!(collective.name.len(), 255);

    db.close().unwrap();
}

#[test]
fn test_create_collective_with_owner_empty_owner_rejected() {
    let (db, _dir) = open_db();

    let result = db.create_collective_with_owner("valid-name", "");
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    db.close().unwrap();
}

// ============================================================================
// Get Collective
// ============================================================================

#[test]
fn test_get_collective() {
    let (db, _dir) = open_db();

    let id = db.create_collective("test-proj").unwrap();

    let collective = db.get_collective(id).unwrap();
    assert!(collective.is_some());
    assert_eq!(collective.unwrap().name, "test-proj");

    db.close().unwrap();
}

#[test]
fn test_get_collective_nonexistent() {
    let (db, _dir) = open_db();

    let result = db.get_collective(CollectiveId::new()).unwrap();
    assert!(result.is_none());

    db.close().unwrap();
}

// ============================================================================
// List Collectives
// ============================================================================

#[test]
fn test_list_collectives() {
    let (db, _dir) = open_db();

    db.create_collective("alpha").unwrap();
    db.create_collective("beta").unwrap();
    db.create_collective("gamma").unwrap();

    let collectives = db.list_collectives().unwrap();
    assert_eq!(collectives.len(), 3);

    let names: Vec<&str> = collectives.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"alpha"));
    assert!(names.contains(&"beta"));
    assert!(names.contains(&"gamma"));

    db.close().unwrap();
}

#[test]
fn test_list_collectives_empty() {
    let (db, _dir) = open_db();

    let collectives = db.list_collectives().unwrap();
    assert!(collectives.is_empty());

    db.close().unwrap();
}

#[test]
fn test_list_collectives_by_owner() {
    let (db, _dir) = open_db();

    db.create_collective_with_owner("proj-a", "alice").unwrap();
    db.create_collective_with_owner("proj-b", "bob").unwrap();
    db.create_collective_with_owner("proj-c", "alice").unwrap();
    db.create_collective("unowned").unwrap();

    let alice = db.list_collectives_by_owner("alice").unwrap();
    assert_eq!(alice.len(), 2);

    let bob = db.list_collectives_by_owner("bob").unwrap();
    assert_eq!(bob.len(), 1);
    assert_eq!(bob[0].name, "proj-b");

    let nobody = db.list_collectives_by_owner("nobody").unwrap();
    assert!(nobody.is_empty());

    db.close().unwrap();
}

// ============================================================================
// Get Collective Stats
// ============================================================================

#[test]
fn test_get_collective_stats() {
    let (db, _dir) = open_db();

    let id = db.create_collective("stats-test").unwrap();

    let stats = db.get_collective_stats(id).unwrap();
    assert_eq!(stats.experience_count, 0);
    assert_eq!(stats.storage_bytes, 0);
    assert!(stats.oldest_experience.is_none());
    assert!(stats.newest_experience.is_none());

    db.close().unwrap();
}

#[test]
fn test_get_collective_stats_nonexistent() {
    let (db, _dir) = open_db();

    let result = db.get_collective_stats(CollectiveId::new());
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());

    db.close().unwrap();
}

// ============================================================================
// Delete Collective
// ============================================================================

#[test]
fn test_delete_collective() {
    let (db, _dir) = open_db();

    let id = db.create_collective("to-delete").unwrap();
    assert!(db.get_collective(id).unwrap().is_some());

    db.delete_collective(id).unwrap();

    assert!(db.get_collective(id).unwrap().is_none());

    db.close().unwrap();
}

#[test]
fn test_delete_collective_nonexistent() {
    let (db, _dir) = open_db();

    let result = db.delete_collective(CollectiveId::new());
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());

    db.close().unwrap();
}

#[test]
fn test_delete_collective_removes_from_list() {
    let (db, _dir) = open_db();

    let id1 = db.create_collective("keep").unwrap();
    let id2 = db.create_collective("remove").unwrap();

    assert_eq!(db.list_collectives().unwrap().len(), 2);

    db.delete_collective(id2).unwrap();

    let remaining = db.list_collectives().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, id1);

    db.close().unwrap();
}

#[test]
fn test_delete_collective_cascades() {
    // Verifies the cascade structure works with 0 experiences.
    // When E1-S03 lands, this test should be extended with actual experiences.
    let (db, _dir) = open_db();

    let id = db.create_collective("cascade-test").unwrap();

    let stats = db.get_collective_stats(id).unwrap();
    assert_eq!(stats.experience_count, 0);

    db.delete_collective(id).unwrap();
    assert!(db.get_collective(id).unwrap().is_none());

    db.close().unwrap();
}

// ============================================================================
// Embedding Dimension Lock
// ============================================================================

#[test]
fn test_embedding_dimension_locked_to_config() {
    let (db, _dir) = open_db();

    let id = db.create_collective("dim-test").unwrap();

    let collective = db.get_collective(id).unwrap().unwrap();
    assert_eq!(collective.embedding_dimension, 384); // default D384

    db.close().unwrap();
}

#[test]
fn test_embedding_dimension_matches_custom_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    let config = Config {
        embedding_dimension: EmbeddingDimension::D768,
        ..Default::default()
    };

    let db = PulseDB::open(&path, config).unwrap();
    let id = db.create_collective("dim-768").unwrap();

    let collective = db.get_collective(id).unwrap().unwrap();
    assert_eq!(collective.embedding_dimension, 768);

    db.close().unwrap();
}

// ============================================================================
// Persistence
// ============================================================================

#[test]
fn test_collectives_persist_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    // Create and populate
    let id = {
        let db = PulseDB::open(&path, Config::default()).unwrap();
        let id = db
            .create_collective_with_owner("persistent", "owner-1")
            .unwrap();
        db.close().unwrap();
        id
    };

    // Reopen and verify
    let db = PulseDB::open(&path, Config::default()).unwrap();
    let collective = db.get_collective(id).unwrap().unwrap();
    assert_eq!(collective.name, "persistent");
    assert_eq!(collective.owner_id.as_deref(), Some("owner-1"));
    assert_eq!(collective.embedding_dimension, 384);

    db.close().unwrap();
}
