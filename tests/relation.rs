//! Integration tests for experience relations (E3-S01).
//!
//! Tests the full stack: PulseDB facade -> validation -> StorageEngine -> redb.
//! Covers relation CRUD, direction-based querying, cascade deletes, and
//! validation error paths.

use pulsedb::{
    CollectiveId, Config, NewExperience, NewExperienceRelation, PulseDB, RelationDirection,
    RelationType,
};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Creates a dummy embedding of the correct dimension.
fn dummy_embedding() -> Vec<f32> {
    vec![0.1; DIM]
}

/// Helper to open a fresh database with default config (External provider, D384).
fn open_db() -> (PulseDB, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = PulseDB::open(&path, Config::default()).unwrap();
    (db, dir)
}

/// Helper: open DB, create a collective, return both IDs.
fn open_db_with_collective() -> (PulseDB, CollectiveId, tempfile::TempDir) {
    let (db, dir) = open_db();
    let cid = db.create_collective("test-collective").unwrap();
    (db, cid, dir)
}

/// Helper to build a minimal valid NewExperience for a given collective.
fn minimal_experience(collective_id: CollectiveId) -> NewExperience {
    NewExperience {
        collective_id,
        content: "Test experience content".to_string(),
        embedding: Some(dummy_embedding()),
        ..Default::default()
    }
}

// ============================================================================
// Store + Get Roundtrip
// ============================================================================

#[test]
fn test_store_and_get_relation() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "Second experience".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    let rel_id = db
        .store_relation(NewExperienceRelation {
            source_id: exp_a,
            target_id: exp_b,
            relation_type: RelationType::Supports,
            strength: 0.8,
            metadata: Some("test metadata".to_string()),
        })
        .unwrap();

    let relation = db.get_relation(rel_id).unwrap().unwrap();
    assert_eq!(relation.id, rel_id);
    assert_eq!(relation.source_id, exp_a);
    assert_eq!(relation.target_id, exp_b);
    assert_eq!(relation.relation_type, RelationType::Supports);
    assert!((relation.strength - 0.8).abs() < f32::EPSILON);
    assert_eq!(relation.metadata, Some("test metadata".to_string()));
}

#[test]
fn test_get_relation_nonexistent() {
    let (db, _dir) = open_db();
    let fake_id = pulsedb::RelationId::new();

    let result = db.get_relation(fake_id).unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Direction-based querying
// ============================================================================

#[test]
fn test_get_related_outgoing() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "Target experience".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Elaborates,
        strength: 0.7,
        metadata: None,
    })
    .unwrap();

    // Outgoing from A should find B
    let related = db
        .get_related_experiences(exp_a, RelationDirection::Outgoing)
        .unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].0.id, exp_b);
    assert_eq!(related[0].1.relation_type, RelationType::Elaborates);

    // Outgoing from B should be empty (B is target, not source)
    let related = db
        .get_related_experiences(exp_b, RelationDirection::Outgoing)
        .unwrap();
    assert!(related.is_empty());
}

#[test]
fn test_get_related_incoming() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "Target experience".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Implies,
        strength: 0.5,
        metadata: None,
    })
    .unwrap();

    // Incoming to B should find A
    let related = db
        .get_related_experiences(exp_b, RelationDirection::Incoming)
        .unwrap();
    assert_eq!(related.len(), 1);
    assert_eq!(related[0].0.id, exp_a);

    // Incoming to A should be empty
    let related = db
        .get_related_experiences(exp_a, RelationDirection::Incoming)
        .unwrap();
    assert!(related.is_empty());
}

#[test]
fn test_get_related_both() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "B experience".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();
    let exp_c = db
        .record_experience(NewExperience {
            content: "C experience".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    // A -> B (outgoing from A)
    db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Supports,
        strength: 0.9,
        metadata: None,
    })
    .unwrap();

    // C -> A (incoming to A)
    db.store_relation(NewExperienceRelation {
        source_id: exp_c,
        target_id: exp_a,
        relation_type: RelationType::Contradicts,
        strength: 0.6,
        metadata: None,
    })
    .unwrap();

    // Both for A should find B (outgoing) and C (incoming)
    let related = db
        .get_related_experiences(exp_a, RelationDirection::Both)
        .unwrap();
    assert_eq!(related.len(), 2);

    let exp_ids: Vec<_> = related.iter().map(|(e, _)| e.id).collect();
    assert!(exp_ids.contains(&exp_b));
    assert!(exp_ids.contains(&exp_c));
}

// ============================================================================
// Delete relation
// ============================================================================

#[test]
fn test_delete_relation() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "Second".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    let rel_id = db
        .store_relation(NewExperienceRelation {
            source_id: exp_a,
            target_id: exp_b,
            relation_type: RelationType::RelatedTo,
            strength: 0.5,
            metadata: None,
        })
        .unwrap();

    // Relation exists
    assert!(db.get_relation(rel_id).unwrap().is_some());

    // Delete it
    db.delete_relation(rel_id).unwrap();

    // Gone
    assert!(db.get_relation(rel_id).unwrap().is_none());

    // Related experiences query returns empty
    let related = db
        .get_related_experiences(exp_a, RelationDirection::Outgoing)
        .unwrap();
    assert!(related.is_empty());
}

// ============================================================================
// Cascade deletes
// ============================================================================

#[test]
fn test_relation_cascade_delete_experience() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "B".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    let rel_id = db
        .store_relation(NewExperienceRelation {
            source_id: exp_a,
            target_id: exp_b,
            relation_type: RelationType::Supersedes,
            strength: 1.0,
            metadata: None,
        })
        .unwrap();

    // Delete the source experience — relation should be cascade-deleted
    db.delete_experience(exp_a).unwrap();

    // Relation is gone
    assert!(db.get_relation(rel_id).unwrap().is_none());

    // Target experience still exists
    assert!(db.get_experience(exp_b).unwrap().is_some());
}

#[test]
fn test_collective_cascade_deletes_relations() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "B".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    let rel_id = db
        .store_relation(NewExperienceRelation {
            source_id: exp_a,
            target_id: exp_b,
            relation_type: RelationType::Supports,
            strength: 0.5,
            metadata: None,
        })
        .unwrap();

    // Delete the entire collective — everything should be gone
    db.delete_collective(cid).unwrap();

    // Relation is gone
    assert!(db.get_relation(rel_id).unwrap().is_none());

    // Experiences are gone
    assert!(db.get_experience(exp_a).unwrap().is_none());
    assert!(db.get_experience(exp_b).unwrap().is_none());
}

// ============================================================================
// Validation error paths
// ============================================================================

#[test]
fn test_self_relation_rejected() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();

    let result = db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_a, // Same ID — self-relation
        relation_type: RelationType::Supports,
        strength: 0.5,
        metadata: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_cross_collective_rejected() {
    let (db, _dir) = open_db();

    let cid_a = db.create_collective("collective-a").unwrap();
    let cid_b = db.create_collective("collective-b").unwrap();

    let exp_a = db.record_experience(minimal_experience(cid_a)).unwrap();
    let exp_b = db.record_experience(minimal_experience(cid_b)).unwrap();

    let result = db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::RelatedTo,
        strength: 0.5,
        metadata: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_duplicate_relation_rejected() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "B".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    // First store succeeds
    db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Supports,
        strength: 0.5,
        metadata: None,
    })
    .unwrap();

    // Same (source, target, type) rejected
    let result = db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Supports,
        strength: 0.9, // Different strength doesn't matter
        metadata: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_relation_different_type_allowed() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "B".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();

    // Supports relation
    db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Supports,
        strength: 0.5,
        metadata: None,
    })
    .unwrap();

    // Contradicts relation between same pair — different type, should succeed
    db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: exp_b,
        relation_type: RelationType::Contradicts,
        strength: 0.3,
        metadata: None,
    })
    .unwrap();

    let related = db
        .get_related_experiences(exp_a, RelationDirection::Outgoing)
        .unwrap();
    assert_eq!(related.len(), 2);
}

#[test]
fn test_relation_nonexistent_source() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_b = db.record_experience(minimal_experience(cid)).unwrap();
    let fake_id = pulsedb::ExperienceId::new();

    let result = db.store_relation(NewExperienceRelation {
        source_id: fake_id,
        target_id: exp_b,
        relation_type: RelationType::Supports,
        strength: 0.5,
        metadata: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn test_relation_nonexistent_target() {
    let (db, cid, _dir) = open_db_with_collective();

    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let fake_id = pulsedb::ExperienceId::new();

    let result = db.store_relation(NewExperienceRelation {
        source_id: exp_a,
        target_id: fake_id,
        relation_type: RelationType::Supports,
        strength: 0.5,
        metadata: None,
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}
