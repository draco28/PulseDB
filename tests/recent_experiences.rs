//! Integration tests for recent experiences API (E2-S03, Ticket #9).
//!
//! Tests the full stack: PulseDB facade → StorageEngine → redb reverse scan.
//! Verifies timestamp ordering, filtering, archival exclusion, and input validation.

use pulsedb::{
    CollectiveId, Config, ExperienceType, NewExperience, PulseDB, SearchFilter, Severity,
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

/// Helper: record N minimal experiences with sequential content.
/// Returns the IDs in creation order (oldest first).
fn record_n_experiences(
    db: &PulseDB,
    cid: CollectiveId,
    count: usize,
) -> Vec<pulsedb::ExperienceId> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        // Small sleep to ensure distinct timestamps between experiences.
        // UUID v7 has millisecond precision, so 2ms spacing is sufficient.
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let id = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: format!("Experience number {}", i),
                embedding: Some(dummy_embedding()),
                ..Default::default()
            })
            .unwrap();
        ids.push(id);
    }
    ids
}

// ============================================================================
// Ordering Tests
// ============================================================================

#[test]
fn test_get_recent_ordered_by_time() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 5 experiences with distinct timestamps
    let ids = record_n_experiences(&db, cid, 5);

    // Get recent: should be newest first (reverse of creation order)
    let recent = db.get_recent_experiences(cid, 10).unwrap();
    assert_eq!(recent.len(), 5);

    // Verify newest-first ordering
    let recent_ids: Vec<_> = recent.iter().map(|e| e.id).collect();
    let mut expected = ids.clone();
    expected.reverse();
    assert_eq!(recent_ids, expected);

    // Verify timestamps are strictly descending
    for window in recent.windows(2) {
        assert!(
            window[0].timestamp >= window[1].timestamp,
            "Expected descending timestamps: {:?} >= {:?}",
            window[0].timestamp,
            window[1].timestamp,
        );
    }
}

// ============================================================================
// Limit Tests
// ============================================================================

#[test]
fn test_recent_respects_limit() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 10 experiences
    record_n_experiences(&db, cid, 10);

    // Request only 3
    let recent = db.get_recent_experiences(cid, 3).unwrap();
    assert_eq!(recent.len(), 3);

    // Request more than exist
    let recent = db.get_recent_experiences(cid, 100).unwrap();
    assert_eq!(recent.len(), 10);
}

#[test]
fn test_recent_invalid_limit_zero() {
    let (db, cid, _dir) = open_db_with_collective();

    let result = db.get_recent_experiences(cid, 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_recent_invalid_limit_too_large() {
    let (db, cid, _dir) = open_db_with_collective();

    let result = db.get_recent_experiences(cid, 1001);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_recent_limit_boundary_1000() {
    let (db, cid, _dir) = open_db_with_collective();

    // limit=1000 should be valid
    let result = db.get_recent_experiences(cid, 1000);
    assert!(result.is_ok());
}

// ============================================================================
// Archival Tests
// ============================================================================

#[test]
fn test_recent_excludes_archived() {
    let (db, cid, _dir) = open_db_with_collective();

    let ids = record_n_experiences(&db, cid, 5);

    // Archive the 2 newest experiences
    db.archive_experience(ids[3]).unwrap();
    db.archive_experience(ids[4]).unwrap();

    // Get recent (default filter excludes archived)
    let recent = db.get_recent_experiences(cid, 10).unwrap();
    assert_eq!(recent.len(), 3);

    // Verify none of the returned experiences are archived
    for exp in &recent {
        assert!(!exp.archived, "Archived experience should be excluded");
    }
}

#[test]
fn test_recent_includes_archived_when_filter_allows() {
    let (db, cid, _dir) = open_db_with_collective();

    let ids = record_n_experiences(&db, cid, 3);

    // Archive one
    db.archive_experience(ids[1]).unwrap();

    // Use filter that includes archived
    let filter = SearchFilter {
        exclude_archived: false,
        ..SearchFilter::default()
    };
    let recent = db.get_recent_experiences_filtered(cid, 10, filter).unwrap();
    assert_eq!(recent.len(), 3);
}

// ============================================================================
// Domain Filter Tests
// ============================================================================

#[test]
fn test_recent_with_domain_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record experiences with different domains
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Rust experience".to_string(),
        embedding: Some(dummy_embedding()),
        domain: vec!["rust".to_string()],
        ..Default::default()
    })
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Python experience".to_string(),
        embedding: Some(dummy_embedding()),
        domain: vec!["python".to_string()],
        ..Default::default()
    })
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Another Rust experience".to_string(),
        embedding: Some(dummy_embedding()),
        domain: vec!["rust".to_string(), "testing".to_string()],
        ..Default::default()
    })
    .unwrap();

    // Filter by "rust" domain
    let filter = SearchFilter {
        domains: Some(vec!["rust".to_string()]),
        ..SearchFilter::default()
    };
    let recent = db.get_recent_experiences_filtered(cid, 10, filter).unwrap();
    assert_eq!(recent.len(), 2);

    // All returned should have "rust" in domain
    for exp in &recent {
        assert!(
            exp.domain.contains(&"rust".to_string()),
            "Expected 'rust' domain, got {:?}",
            exp.domain,
        );
    }
}

// ============================================================================
// Type Filter Tests
// ============================================================================

#[test]
fn test_recent_with_type_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record a Fact
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Rust is memory safe".to_string(),
        experience_type: ExperienceType::Fact {
            statement: "Rust is memory safe".to_string(),
            source: String::new(),
        },
        embedding: Some(dummy_embedding()),
        ..Default::default()
    })
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    // Record a Difficulty
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Lifetime errors are confusing".to_string(),
        experience_type: ExperienceType::Difficulty {
            description: "Lifetime errors".to_string(),
            severity: Severity::Medium,
        },
        embedding: Some(dummy_embedding()),
        ..Default::default()
    })
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    // Record another Fact
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Tokio is an async runtime".to_string(),
        experience_type: ExperienceType::Fact {
            statement: "Tokio is an async runtime".to_string(),
            source: "docs.rs".to_string(),
        },
        embedding: Some(dummy_embedding()),
        ..Default::default()
    })
    .unwrap();

    // Filter by Fact type only
    let filter = SearchFilter {
        experience_types: Some(vec![ExperienceType::Fact {
            statement: String::new(),
            source: String::new(),
        }]),
        ..SearchFilter::default()
    };
    let recent = db.get_recent_experiences_filtered(cid, 10, filter).unwrap();
    assert_eq!(recent.len(), 2);

    // All returned should be Fact type
    for exp in &recent {
        assert!(
            matches!(exp.experience_type, ExperienceType::Fact { .. }),
            "Expected Fact type, got {:?}",
            exp.experience_type,
        );
    }
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_recent_empty_collective() {
    let (db, cid, _dir) = open_db_with_collective();

    // No experiences recorded — should return empty vec, not error
    let recent = db.get_recent_experiences(cid, 10).unwrap();
    assert!(recent.is_empty());
}

#[test]
fn test_recent_collective_not_found() {
    let (db, _dir) = open_db();

    let fake_id = CollectiveId::new();
    let result = db.get_recent_experiences(fake_id, 10);
    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

// ============================================================================
// Importance Filter
// ============================================================================

#[test]
fn test_recent_with_importance_filter() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record with varying importance
    db.record_experience(NewExperience {
        collective_id: cid,
        content: "Low importance".to_string(),
        embedding: Some(dummy_embedding()),
        importance: 0.2,
        ..Default::default()
    })
    .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));

    db.record_experience(NewExperience {
        collective_id: cid,
        content: "High importance".to_string(),
        embedding: Some(dummy_embedding()),
        importance: 0.9,
        ..Default::default()
    })
    .unwrap();

    let filter = SearchFilter {
        min_importance: Some(0.5),
        ..SearchFilter::default()
    };
    let recent = db.get_recent_experiences_filtered(cid, 10, filter).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].content, "High importance");
}

// ============================================================================
// Collective Isolation
// ============================================================================

#[test]
fn test_recent_collective_isolation() {
    let (db, _dir) = open_db();

    let cid1 = db.create_collective("collective-1").unwrap();
    let cid2 = db.create_collective("collective-2").unwrap();

    // Record 3 in collective 1
    record_n_experiences(&db, cid1, 3);

    // Record 2 in collective 2
    record_n_experiences(&db, cid2, 2);

    // Only collective 1's experiences returned
    let recent1 = db.get_recent_experiences(cid1, 10).unwrap();
    assert_eq!(recent1.len(), 3);
    for exp in &recent1 {
        assert_eq!(exp.collective_id, cid1);
    }

    // Only collective 2's experiences returned
    let recent2 = db.get_recent_experiences(cid2, 10).unwrap();
    assert_eq!(recent2.len(), 2);
    for exp in &recent2 {
        assert_eq!(exp.collective_id, cid2);
    }
}
