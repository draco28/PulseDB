//! Integration tests for similarity search API (E2-S02, Ticket #8).
//!
//! Tests the full stack: PulseDB facade → HNSW search → StorageEngine fetch.
//! Verifies result ordering, filtering, archival exclusion, dimension validation,
//! and collective isolation.

use pulsedb::{
    CollectiveId, Config, ExperienceType, NewExperience, PulseDB, SearchFilter, Severity, Timestamp,
};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Generates a deterministic embedding from a seed.
/// Vectors with close seeds produce similar embeddings (smooth sin curve).
fn make_embedding(seed: u64) -> Vec<f32> {
    (0..DIM)
        .map(|i| (seed as f32 * 0.1 + i as f32 * 0.01).sin())
        .collect()
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

/// Helper: record experiences with distinct embeddings (different seeds).
/// Returns the IDs in creation order.
fn record_experiences_with_embeddings(
    db: &PulseDB,
    cid: CollectiveId,
    seeds: &[u64],
) -> Vec<pulsedb::ExperienceId> {
    seeds
        .iter()
        .map(|&seed| {
            db.record_experience(NewExperience {
                collective_id: cid,
                content: format!("Experience seed={}", seed),
                embedding: Some(make_embedding(seed)),
                ..Default::default()
            })
            .unwrap()
        })
        .collect()
}

// ============================================================================
// Result Ordering
// ============================================================================

#[test]
fn test_search_similar_returns_sorted() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record experiences with seeds 0..10
    record_experiences_with_embeddings(&db, cid, &(0..10).collect::<Vec<_>>());

    // Search with a query similar to seed=5
    let query = make_embedding(5);
    let results = db.search_similar(cid, &query, 5).unwrap();

    assert!(!results.is_empty());
    assert!(results.len() <= 5);

    // Results should be sorted by similarity descending
    for window in results.windows(2) {
        assert!(
            window[0].similarity >= window[1].similarity,
            "Results not sorted by similarity: {} < {}",
            window[0].similarity,
            window[1].similarity,
        );
    }

    // The most similar result should have high similarity (close to 1.0)
    assert!(
        results[0].similarity > 0.5,
        "Expected high similarity for matching vector, got {}",
        results[0].similarity,
    );
}

// ============================================================================
// Archival Exclusion
// ============================================================================

#[test]
fn test_search_excludes_archived() {
    let (db, cid, _dir) = open_db_with_collective();

    // Use 10 experiences for reliable HNSW graph connectivity
    let seeds: Vec<u64> = (0..10).collect();
    let ids = record_experiences_with_embeddings(&db, cid, &seeds);

    // Archive 2 experiences
    db.archive_experience(ids[3]).unwrap();
    db.archive_experience(ids[7]).unwrap();

    let query = make_embedding(5);
    let results = db.search_similar(cid, &query, 20).unwrap();

    // No archived experiences should appear in results
    let result_ids: Vec<_> = results.iter().map(|r| r.experience.id).collect();
    assert!(
        !result_ids.contains(&ids[3]),
        "Archived experience ids[3] should be excluded"
    );
    assert!(
        !result_ids.contains(&ids[7]),
        "Archived experience ids[7] should be excluded"
    );
    for r in &results {
        assert!(!r.experience.archived, "No archived experiences in results");
    }

    // Should have 8 results (10 - 2 archived)
    assert_eq!(results.len(), 8);
}

// ============================================================================
// Domain Filter
// ============================================================================

#[test]
fn test_search_respects_domain_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 5 rust experiences and 5 python experiences for reliable HNSW graph
    for seed in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Rust experience {}", seed),
            embedding: Some(make_embedding(seed)),
            domain: vec!["rust".to_string()],
            ..Default::default()
        })
        .unwrap();
    }

    for seed in 5..10u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Python experience {}", seed),
            embedding: Some(make_embedding(seed)),
            domain: vec!["python".to_string()],
            ..Default::default()
        })
        .unwrap();
    }

    let filter = SearchFilter {
        domains: Some(vec!["rust".to_string()]),
        ..SearchFilter::default()
    };
    let query = make_embedding(2); // query among rust experiences
    let results = db.search_similar_filtered(cid, &query, 20, filter).unwrap();

    assert_eq!(results.len(), 5, "Should find all 5 rust experiences");
    for r in &results {
        assert!(
            r.experience.domain.contains(&"rust".to_string()),
            "Expected 'rust' domain, got {:?}",
            r.experience.domain,
        );
    }
}

// ============================================================================
// Importance Filter
// ============================================================================

#[test]
fn test_search_respects_importance_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // 5 low importance, 5 high importance
    for seed in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Low importance {}", seed),
            embedding: Some(make_embedding(seed)),
            importance: 0.2,
            ..Default::default()
        })
        .unwrap();
    }

    for seed in 5..10u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("High importance {}", seed),
            embedding: Some(make_embedding(seed)),
            importance: 0.9,
            ..Default::default()
        })
        .unwrap();
    }

    let filter = SearchFilter {
        min_importance: Some(0.5),
        ..SearchFilter::default()
    };
    let query = make_embedding(7);
    let results = db.search_similar_filtered(cid, &query, 20, filter).unwrap();

    assert_eq!(
        results.len(),
        5,
        "Should find all 5 high importance experiences"
    );
    for r in &results {
        assert!(
            r.experience.importance >= 0.5,
            "Expected importance >= 0.5, got {}",
            r.experience.importance,
        );
    }
}

// ============================================================================
// Confidence Filter
// ============================================================================

#[test]
fn test_search_respects_confidence_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // 5 low confidence, 5 high confidence
    for seed in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Low confidence {}", seed),
            embedding: Some(make_embedding(seed)),
            confidence: 0.3,
            ..Default::default()
        })
        .unwrap();
    }

    for seed in 5..10u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("High confidence {}", seed),
            embedding: Some(make_embedding(seed)),
            confidence: 0.9,
            ..Default::default()
        })
        .unwrap();
    }

    let filter = SearchFilter {
        min_confidence: Some(0.7),
        ..SearchFilter::default()
    };
    let query = make_embedding(7);
    let results = db.search_similar_filtered(cid, &query, 20, filter).unwrap();

    assert_eq!(
        results.len(),
        5,
        "Should find all 5 high confidence experiences"
    );
    for r in &results {
        assert!(
            r.experience.confidence >= 0.7,
            "Expected confidence >= 0.7, got {}",
            r.experience.confidence,
        );
    }
}

// ============================================================================
// Type Filter
// ============================================================================

#[test]
fn test_search_respects_type_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // 5 Fact experiences, 5 Difficulty experiences
    for seed in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Fact {}", seed),
            experience_type: ExperienceType::Fact {
                statement: format!("Fact statement {}", seed),
                source: String::new(),
            },
            embedding: Some(make_embedding(seed)),
            ..Default::default()
        })
        .unwrap();
    }

    for seed in 5..10u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Difficulty {}", seed),
            experience_type: ExperienceType::Difficulty {
                description: format!("Problem {}", seed),
                severity: Severity::Medium,
            },
            embedding: Some(make_embedding(seed)),
            ..Default::default()
        })
        .unwrap();
    }

    let filter = SearchFilter {
        experience_types: Some(vec![ExperienceType::Fact {
            statement: String::new(),
            source: String::new(),
        }]),
        ..SearchFilter::default()
    };
    let query = make_embedding(2);
    let results = db.search_similar_filtered(cid, &query, 20, filter).unwrap();

    assert_eq!(results.len(), 5, "Should find all 5 Fact experiences");
    for r in &results {
        assert!(
            matches!(r.experience.experience_type, ExperienceType::Fact { .. }),
            "Expected Fact type, got {:?}",
            r.experience.experience_type,
        );
    }
}

// ============================================================================
// Since (Timestamp) Filter
// ============================================================================

#[test]
fn test_search_respects_since_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 5 experiences before cutoff
    for seed in 0..5u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Old experience {}", seed),
            embedding: Some(make_embedding(seed)),
            ..Default::default()
        })
        .unwrap();
    }

    std::thread::sleep(std::time::Duration::from_millis(10));
    let cutoff = Timestamp::now();
    std::thread::sleep(std::time::Duration::from_millis(10));

    // Record 5 experiences after cutoff
    for seed in 5..10u64 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("New experience {}", seed),
            embedding: Some(make_embedding(seed)),
            ..Default::default()
        })
        .unwrap();
    }

    let filter = SearchFilter {
        since: Some(cutoff),
        ..SearchFilter::default()
    };
    let query = make_embedding(7); // query among the "new" experiences
    let results = db.search_similar_filtered(cid, &query, 20, filter).unwrap();

    // All results should be after cutoff
    assert!(!results.is_empty(), "Should find experiences after cutoff");
    for r in &results {
        assert!(
            r.experience.timestamp >= cutoff,
            "Experience should be after cutoff"
        );
        assert!(
            r.experience.content.starts_with("New"),
            "Expected new experience, got: {}",
            r.experience.content
        );
    }
    assert_eq!(results.len(), 5, "Should find all 5 new experiences");
}

// ============================================================================
// Collective Isolation
// ============================================================================

#[test]
fn test_search_collective_isolation() {
    let (db, _dir) = open_db();

    let cid1 = db.create_collective("collective-1").unwrap();
    let cid2 = db.create_collective("collective-2").unwrap();

    // Record 3 in collective 1
    record_experiences_with_embeddings(&db, cid1, &[1, 2, 3]);

    // Record 2 in collective 2
    record_experiences_with_embeddings(&db, cid2, &[4, 5]);

    // Search collective 1 — should only find its 3 experiences
    let query = make_embedding(1);
    let results1 = db.search_similar(cid1, &query, 10).unwrap();
    assert_eq!(results1.len(), 3);
    for r in &results1 {
        assert_eq!(r.experience.collective_id, cid1);
    }

    // Search collective 2 — should only find its 2 experiences
    let query = make_embedding(4);
    let results2 = db.search_similar(cid2, &query, 10).unwrap();
    assert_eq!(results2.len(), 2);
    for r in &results2 {
        assert_eq!(r.experience.collective_id, cid2);
    }
}

// ============================================================================
// Input Validation
// ============================================================================

#[test]
fn test_search_dimension_mismatch_error() {
    let (db, cid, _dir) = open_db_with_collective();

    // Query with wrong dimension (128 instead of 384)
    let wrong_query = vec![0.1f32; 128];
    let result = db.search_similar(cid, &wrong_query, 10);

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.is_validation());
    assert!(err.to_string().contains("dimension"));
}

#[test]
fn test_search_invalid_k() {
    let (db, cid, _dir) = open_db_with_collective();

    let query = make_embedding(1);

    // k=0 should fail
    let result = db.search_similar(cid, &query, 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    // k=1001 should fail
    let result = db.search_similar(cid, &query, 1001);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    // k=1000 should succeed (boundary)
    let result = db.search_similar(cid, &query, 1000);
    assert!(result.is_ok());
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_search_empty_index() {
    let (db, cid, _dir) = open_db_with_collective();

    // Search with no experiences recorded
    let query = make_embedding(1);
    let results = db.search_similar(cid, &query, 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_search_collective_not_found() {
    let (db, _dir) = open_db();

    let fake_id = CollectiveId::new();
    let query = make_embedding(1);
    let result = db.search_similar(fake_id, &query, 10);

    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}
