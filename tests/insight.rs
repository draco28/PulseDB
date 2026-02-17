//! Integration tests for derived insights (E3-S02).
//!
//! Tests the full stack: PulseDB facade -> validation -> StorageEngine -> redb -> HNSW.
//! Covers insight CRUD, vector search, cascade deletes, and validation error paths.

use pulsedb::{
    CollectiveId, Config, ExperienceId, InsightId, InsightType, NewDerivedInsight, NewExperience,
    PulseDB,
};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Creates a dummy embedding of the correct dimension.
fn dummy_embedding() -> Vec<f32> {
    vec![0.1; DIM]
}

/// Creates a distinct embedding for vector search differentiation.
fn distinct_embedding(seed: f32) -> Vec<f32> {
    (0..DIM).map(|i| (i as f32 * seed).sin()).collect()
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

/// Helper: record two experiences and return their IDs (used as insight sources).
fn record_source_experiences(db: &PulseDB, cid: CollectiveId) -> (ExperienceId, ExperienceId) {
    let exp_a = db.record_experience(minimal_experience(cid)).unwrap();
    let exp_b = db
        .record_experience(NewExperience {
            content: "Second experience".to_string(),
            ..minimal_experience(cid)
        })
        .unwrap();
    (exp_a, exp_b)
}

// ============================================================================
// Store + Get Roundtrip
// ============================================================================

#[test]
fn test_store_insight() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, exp_b) = record_source_experiences(&db, cid);

    let insight_id = db
        .store_insight(NewDerivedInsight {
            collective_id: cid,
            content: "Error handling patterns converge on early return".to_string(),
            embedding: Some(dummy_embedding()),
            source_experience_ids: vec![exp_a, exp_b],
            insight_type: InsightType::Pattern,
            confidence: 0.85,
            domain: vec!["rust".to_string(), "error-handling".to_string()],
        })
        .unwrap();

    let insight = db.get_insight(insight_id).unwrap().unwrap();
    assert_eq!(insight.id, insight_id);
    assert_eq!(insight.collective_id, cid);
    assert_eq!(
        insight.content,
        "Error handling patterns converge on early return"
    );
    assert_eq!(insight.embedding.len(), DIM);
    assert_eq!(insight.source_experience_ids.len(), 2);
    assert!(insight.source_experience_ids.contains(&exp_a));
    assert!(insight.source_experience_ids.contains(&exp_b));
    assert_eq!(insight.insight_type, InsightType::Pattern);
    assert!((insight.confidence - 0.85).abs() < f32::EPSILON);
    assert_eq!(insight.domain, vec!["rust", "error-handling"]);
}

#[test]
fn test_get_insight_by_id() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, exp_b) = record_source_experiences(&db, cid);

    let insight_id = db
        .store_insight(NewDerivedInsight {
            collective_id: cid,
            content: "Test insight".to_string(),
            embedding: Some(dummy_embedding()),
            source_experience_ids: vec![exp_a, exp_b],
            insight_type: InsightType::Synthesis,
            confidence: 0.7,
            domain: vec![],
        })
        .unwrap();

    // Found
    let insight = db.get_insight(insight_id).unwrap();
    assert!(insight.is_some());

    // Not found
    let missing = db.get_insight(InsightId::new()).unwrap();
    assert!(missing.is_none());
}

// ============================================================================
// Vector search
// ============================================================================

#[test]
fn test_get_insights_similar() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, exp_b) = record_source_experiences(&db, cid);

    // Store 3 insights with distinct embeddings
    let emb_a = distinct_embedding(1.0);
    let emb_b = distinct_embedding(2.0);
    let emb_c = distinct_embedding(3.0);

    db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Insight A".to_string(),
        embedding: Some(emb_a.clone()),
        source_experience_ids: vec![exp_a, exp_b],
        insight_type: InsightType::Pattern,
        confidence: 0.8,
        domain: vec![],
    })
    .unwrap();

    db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Insight B".to_string(),
        embedding: Some(emb_b),
        source_experience_ids: vec![exp_a],
        insight_type: InsightType::Synthesis,
        confidence: 0.7,
        domain: vec![],
    })
    .unwrap();

    db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Insight C".to_string(),
        embedding: Some(emb_c),
        source_experience_ids: vec![exp_b],
        insight_type: InsightType::Abstraction,
        confidence: 0.9,
        domain: vec![],
    })
    .unwrap();

    // Search with emb_a as query — should find Insight A as most similar
    let results = db.get_insights(cid, &emb_a, 3).unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].content, "Insight A");
}

// ============================================================================
// Delete insight
// ============================================================================

#[test]
fn test_delete_insight() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, exp_b) = record_source_experiences(&db, cid);

    let emb = distinct_embedding(1.0);

    let insight_id = db
        .store_insight(NewDerivedInsight {
            collective_id: cid,
            content: "Deletable insight".to_string(),
            embedding: Some(emb.clone()),
            source_experience_ids: vec![exp_a, exp_b],
            insight_type: InsightType::Correlation,
            confidence: 0.6,
            domain: vec![],
        })
        .unwrap();

    // Exists
    assert!(db.get_insight(insight_id).unwrap().is_some());

    // Delete
    db.delete_insight(insight_id).unwrap();

    // Gone from storage
    assert!(db.get_insight(insight_id).unwrap().is_none());

    // Gone from HNSW (search returns empty)
    let results = db.get_insights(cid, &emb, 5).unwrap();
    assert!(results.is_empty());
}

// ============================================================================
// Validation error paths
// ============================================================================

#[test]
fn test_insight_source_validation() {
    let (db, cid, _dir) = open_db_with_collective();

    // Nonexistent source experience → NotFound
    let fake_id = ExperienceId::new();
    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Bad insight".to_string(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: vec![fake_id],
        insight_type: InsightType::Pattern,
        confidence: 0.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_not_found());
}

#[test]
fn test_insight_cross_collective_rejected() {
    let (db, _dir) = open_db();

    let cid_a = db.create_collective("collective-a").unwrap();
    let cid_b = db.create_collective("collective-b").unwrap();

    // Record experience in collective B
    let exp_b = db.record_experience(minimal_experience(cid_b)).unwrap();

    // Try to create insight in collective A with source from collective B
    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid_a,
        content: "Cross-collective insight".to_string(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: vec![exp_b],
        insight_type: InsightType::Synthesis,
        confidence: 0.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_insight_embedding_dimension_validation() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, _exp_b) = record_source_experiences(&db, cid);

    // Wrong dimension embedding (768 instead of 384)
    let wrong_dim_embedding = vec![0.1; 768];

    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Wrong dimension insight".to_string(),
        embedding: Some(wrong_dim_embedding),
        source_experience_ids: vec![exp_a],
        insight_type: InsightType::Pattern,
        confidence: 0.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_insight_confidence_validation() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, _exp_b) = record_source_experiences(&db, cid);

    // Confidence above 1.0
    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "High confidence insight".to_string(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: vec![exp_a],
        insight_type: InsightType::Pattern,
        confidence: 1.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());

    // Confidence below 0.0
    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Low confidence insight".to_string(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: vec![exp_a],
        insight_type: InsightType::Pattern,
        confidence: -0.1,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_insight_empty_content_rejected() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, _exp_b) = record_source_experiences(&db, cid);

    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: String::new(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: vec![exp_a],
        insight_type: InsightType::Pattern,
        confidence: 0.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_insight_empty_sources_rejected() {
    let (db, cid, _dir) = open_db_with_collective();

    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "No sources insight".to_string(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: vec![],
        insight_type: InsightType::Pattern,
        confidence: 0.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

#[test]
fn test_insight_too_many_sources_rejected() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 101 experiences (one more than max)
    let mut sources = Vec::new();
    for i in 0..101 {
        let exp_id = db
            .record_experience(NewExperience {
                content: format!("Experience {}", i),
                ..minimal_experience(cid)
            })
            .unwrap();
        sources.push(exp_id);
    }

    let result = db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Too many sources".to_string(),
        embedding: Some(dummy_embedding()),
        source_experience_ids: sources,
        insight_type: InsightType::Synthesis,
        confidence: 0.5,
        domain: vec![],
    });

    assert!(result.is_err());
    assert!(result.unwrap_err().is_validation());
}

// ============================================================================
// Cascade delete
// ============================================================================

#[test]
fn test_insight_collective_cascade_delete() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, exp_b) = record_source_experiences(&db, cid);

    let insight_id = db
        .store_insight(NewDerivedInsight {
            collective_id: cid,
            content: "Cascade test insight".to_string(),
            embedding: Some(dummy_embedding()),
            source_experience_ids: vec![exp_a, exp_b],
            insight_type: InsightType::Abstraction,
            confidence: 0.9,
            domain: vec![],
        })
        .unwrap();

    // Verify insight exists
    assert!(db.get_insight(insight_id).unwrap().is_some());

    // Delete the collective — insights should be cascade-deleted
    db.delete_collective(cid).unwrap();

    // Insight is gone
    assert!(db.get_insight(insight_id).unwrap().is_none());

    // Experiences are gone too
    assert!(db.get_experience(exp_a).unwrap().is_none());
    assert!(db.get_experience(exp_b).unwrap().is_none());
}

// ============================================================================
// All InsightType variants
// ============================================================================

#[test]
fn test_all_insight_types() {
    let (db, cid, _dir) = open_db_with_collective();
    let (exp_a, _exp_b) = record_source_experiences(&db, cid);

    let types = [
        InsightType::Pattern,
        InsightType::Synthesis,
        InsightType::Abstraction,
        InsightType::Correlation,
    ];

    for insight_type in &types {
        let id = db
            .store_insight(NewDerivedInsight {
                collective_id: cid,
                content: format!("Insight of type {:?}", insight_type),
                embedding: Some(dummy_embedding()),
                source_experience_ids: vec![exp_a],
                insight_type: *insight_type,
                confidence: 0.5,
                domain: vec![],
            })
            .unwrap();

        let insight = db.get_insight(id).unwrap().unwrap();
        assert_eq!(insight.insight_type, *insight_type);
    }
}
