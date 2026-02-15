//! Integration tests for HNSW vector index integration (E2-S01, Ticket #7).
//!
//! Tests the full stack: PulseDB → HnswIndex lifecycle, including
//! creation, population via record_experience, soft-delete, persistence
//! across reopen, and rebuild from redb embeddings.

use pulsedb::{CollectiveId, Config, NewExperience, PulseDB};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Generates a deterministic embedding from a seed.
///
/// Vectors with close seeds produce similar embeddings (correlated via sin),
/// enabling predictable nearest-neighbor ordering in tests.
fn make_embedding(seed: u64) -> Vec<f32> {
    (0..DIM)
        .map(|i| (seed as f32 * 0.1 + i as f32 * 0.01).sin())
        .collect()
}

/// Helper: open a fresh database with default config.
fn open_db() -> (PulseDB, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = PulseDB::open(&path, Config::default()).unwrap();
    (db, dir)
}

/// Helper: open DB with a collective, return DB + collective ID.
fn open_db_with_collective() -> (PulseDB, CollectiveId, tempfile::TempDir) {
    let (db, dir) = open_db();
    let cid = db.create_collective("test-collective").unwrap();
    (db, cid, dir)
}

// ============================================================================
// Index Created with Collective
// ============================================================================

#[test]
fn test_hnsw_index_created_with_collective() {
    let (db, cid, _dir) = open_db_with_collective();

    // The index should exist but be empty
    let result = db
        .with_vector_index(cid, |idx| Ok(idx.active_count()))
        .unwrap();
    assert_eq!(result, Some(0));

    db.close().unwrap();
}

// ============================================================================
// Index Populated on record_experience
// ============================================================================

#[test]
fn test_hnsw_index_populated_on_record() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 5 experiences with different embeddings
    for i in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Experience {}", i),
            embedding: Some(make_embedding(i)),
            importance: 0.5,
            ..Default::default()
        })
        .unwrap();
    }

    // Verify the HNSW index has 5 entries
    let count = db
        .with_vector_index(cid, |idx| Ok(idx.active_count()))
        .unwrap()
        .unwrap();
    assert_eq!(count, 5);

    db.close().unwrap();
}

// ============================================================================
// Soft-Delete on experience delete
// ============================================================================

#[test]
fn test_hnsw_soft_delete_on_experience_delete() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 3 experiences
    let mut ids = Vec::new();
    for i in 0..3u64 {
        let id = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: format!("Experience {}", i),
                embedding: Some(make_embedding(i)),
                ..Default::default()
            })
            .unwrap();
        ids.push(id);
    }

    assert_eq!(
        db.with_vector_index(cid, |idx| Ok(idx.active_count()))
            .unwrap()
            .unwrap(),
        3
    );

    // Delete the first experience
    db.delete_experience(ids[0]).unwrap();

    // HNSW should now have 2 active vectors
    assert_eq!(
        db.with_vector_index(cid, |idx| Ok(idx.active_count()))
            .unwrap()
            .unwrap(),
        2
    );

    // The deleted experience should not appear in search results
    let search_results = db
        .with_vector_index(cid, |idx| {
            idx.search_experiences(&make_embedding(0), 10, 50)
        })
        .unwrap()
        .unwrap();
    let result_ids: Vec<_> = search_results.iter().map(|(id, _)| *id).collect();
    assert!(!result_ids.contains(&ids[0]));

    db.close().unwrap();
}

// ============================================================================
// Persistence Across Reopen (Rebuild from redb)
// ============================================================================

#[test]
fn test_hnsw_rebuild_on_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    let cid;
    let exp_count;

    // Phase 1: Create database, populate with experiences
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();
        cid = db.create_collective("persist-test").unwrap();

        for i in 0..10u64 {
            db.record_experience(NewExperience {
                collective_id: cid,
                content: format!("Persistent experience {}", i),
                embedding: Some(make_embedding(i)),
                ..Default::default()
            })
            .unwrap();
        }

        exp_count = db
            .with_vector_index(cid, |idx| Ok(idx.active_count()))
            .unwrap()
            .unwrap();
        assert_eq!(exp_count, 10);

        db.close().unwrap();
    }

    // Phase 2: Reopen — HNSW should be rebuilt from redb embeddings
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();

        let rebuilt_count = db
            .with_vector_index(cid, |idx| Ok(idx.active_count()))
            .unwrap()
            .unwrap();
        assert_eq!(rebuilt_count, exp_count, "HNSW index should be rebuilt from redb");

        // Verify search still works after rebuild
        let results = db
            .with_vector_index(cid, |idx| {
                idx.search_experiences(&make_embedding(5), 3, 50)
            })
            .unwrap()
            .unwrap();
        assert!(!results.is_empty(), "Search should return results after rebuild");
        assert!(results.len() <= 3);

        db.close().unwrap();
    }
}

// ============================================================================
// Rebuild When HNSW Files Missing
// ============================================================================

#[test]
fn test_hnsw_rebuild_when_files_missing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");

    let cid;

    // Phase 1: Create and populate
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();
        cid = db.create_collective("missing-files-test").unwrap();

        db.record_experience(NewExperience {
            collective_id: cid,
            content: "Will survive file deletion".to_string(),
            embedding: Some(make_embedding(42)),
            ..Default::default()
        })
        .unwrap();

        db.close().unwrap();
    }

    // Phase 2: Delete HNSW directory (simulate corruption/missing files)
    let hnsw_dir = path.with_extension("db.hnsw");
    if hnsw_dir.exists() {
        std::fs::remove_dir_all(&hnsw_dir).unwrap();
    }

    // Phase 3: Reopen — should rebuild from redb without error
    {
        let db = PulseDB::open(&path, Config::default()).unwrap();

        let count = db
            .with_vector_index(cid, |idx| Ok(idx.active_count()))
            .unwrap()
            .unwrap();
        assert_eq!(count, 1, "Should rebuild 1 vector from redb");

        db.close().unwrap();
    }
}

// ============================================================================
// Removal on Collective Delete
// ============================================================================

#[test]
fn test_hnsw_removed_on_collective_delete() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record an experience
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Will be cascade deleted".to_string(),
        embedding: Some(make_embedding(1)),
        ..Default::default()
    })
    .unwrap();

    // Verify index exists
    assert!(db
        .with_vector_index(cid, |idx| Ok(idx.active_count()))
        .unwrap()
        .is_some());

    // Delete collective (cascades experiences and HNSW index)
    db.delete_collective(cid).unwrap();

    // HNSW index should be gone
    let result = db
        .with_vector_index(cid, |idx| Ok(idx.active_count()))
        .unwrap();
    assert!(result.is_none(), "HNSW index should be removed with collective");

    db.close().unwrap();
}

// ============================================================================
// Multi-Collective Isolation
// ============================================================================

#[test]
fn test_hnsw_multi_collective_isolation() {
    let (db, _dir) = open_db();

    let cid_a = db.create_collective("collective-a").unwrap();
    let cid_b = db.create_collective("collective-b").unwrap();

    // Record in collective A
    for i in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid_a,
            content: format!("A-experience {}", i),
            embedding: Some(make_embedding(i)),
            ..Default::default()
        })
        .unwrap();
    }

    // Record in collective B
    for i in 10..13u64 {
        db.record_experience(NewExperience {
            collective_id: cid_b,
            content: format!("B-experience {}", i),
            embedding: Some(make_embedding(i)),
            ..Default::default()
        })
        .unwrap();
    }

    // Verify counts are independent
    let count_a = db
        .with_vector_index(cid_a, |idx| Ok(idx.active_count()))
        .unwrap()
        .unwrap();
    let count_b = db
        .with_vector_index(cid_b, |idx| Ok(idx.active_count()))
        .unwrap()
        .unwrap();

    assert_eq!(count_a, 5);
    assert_eq!(count_b, 3);

    // Deleting collective A should not affect B
    db.delete_collective(cid_a).unwrap();

    let count_b_after = db
        .with_vector_index(cid_b, |idx| Ok(idx.active_count()))
        .unwrap()
        .unwrap();
    assert_eq!(count_b_after, 3);

    db.close().unwrap();
}

// ============================================================================
// Search Returns Nearest Neighbors
// ============================================================================

#[test]
fn test_hnsw_search_returns_nearest_neighbors() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record experiences with different seeds (different embeddings)
    for i in 0..20u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Experience seed={}", i),
            embedding: Some(make_embedding(i)),
            ..Default::default()
        })
        .unwrap();
    }

    // Search for nearest to seed=10
    let results = db
        .with_vector_index(cid, |idx| {
            idx.search_experiences(&make_embedding(10), 5, 50)
        })
        .unwrap()
        .unwrap();

    assert_eq!(results.len(), 5, "Should return exactly k=5 results");

    // Results should be sorted by distance ascending (closest first)
    for window in results.windows(2) {
        assert!(
            window[0].1 <= window[1].1,
            "Results should be sorted by distance: {} <= {}",
            window[0].1,
            window[1].1
        );
    }

    // The first result should have very small distance (near-identical to query)
    assert!(
        results[0].1 < 0.01,
        "Closest match should have near-zero distance, got {}",
        results[0].1
    );

    db.close().unwrap();
}
