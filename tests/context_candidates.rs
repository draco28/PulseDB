//! Integration tests for unified context retrieval API (E2-S04, Ticket #13).
//!
//! Tests the full orchestration: PulseDB facade → 5 sub-primitives → aggregation.
//! Verifies inclusion/exclusion flags, filtering, validation, and empty collective
//! handling.

use pulsedb::{
    CollectiveId, Config, ContextRequest, ExperienceId, InsightType, NewActivity,
    NewDerivedInsight, NewExperience, NewExperienceRelation, PulseDB, RelationType, SearchFilter,
};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Generates a deterministic embedding from a seed.
///
/// Uses a hash-based pseudo-random generator to produce well-separated vectors
/// in the 384-dimensional space. Adjacent seeds are NOT correlated, which
/// prevents HNSW neighbor pruning issues with highly similar vectors.
fn make_embedding(seed: u64) -> Vec<f32> {
    (0..DIM)
        .map(|i| {
            let h = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(i as u64)
                .wrapping_mul(1442695040888963407);
            (h >> 33) as f32 / (u32::MAX as f32) - 0.5
        })
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

/// Helper: record N experiences with distinct embeddings.
fn record_experiences(db: &PulseDB, cid: CollectiveId, seeds: &[u64]) -> Vec<ExperienceId> {
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
// Inclusion Tests
// ============================================================================

#[test]
fn test_context_includes_similar() {
    let (db, cid, _dir) = open_db_with_collective();
    record_experiences(&db, cid, &(0..10).collect::<Vec<_>>());

    let query = make_embedding(5);
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            max_similar: 5,
            include_insights: false,
            include_relations: false,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    assert!(!candidates.similar_experiences.is_empty());
    assert!(candidates.similar_experiences.len() <= 5);

    // Verify sorted by similarity descending
    assert!(candidates
        .similar_experiences
        .windows(2)
        .all(|w| w[0].similarity >= w[1].similarity));
}

#[test]
fn test_context_includes_recent() {
    let (db, cid, _dir) = open_db_with_collective();
    record_experiences(&db, cid, &(0..10).collect::<Vec<_>>());

    let query = make_embedding(0);
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            max_recent: 5,
            include_insights: false,
            include_relations: false,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    assert!(!candidates.recent_experiences.is_empty());
    assert!(candidates.recent_experiences.len() <= 5);

    // Verify sorted by timestamp descending (newest first)
    assert!(candidates
        .recent_experiences
        .windows(2)
        .all(|w| w[0].timestamp >= w[1].timestamp));
}

#[test]
fn test_context_includes_insights() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_ids = record_experiences(&db, cid, &[100, 200]);

    // Store an insight derived from those experiences
    db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Error handling pattern detected".to_string(),
        embedding: Some(make_embedding(150)),
        source_experience_ids: exp_ids,
        insight_type: InsightType::Pattern,
        confidence: 0.9,
        domain: vec!["rust".to_string()],
    })
    .unwrap();

    let query = make_embedding(150); // Similar to the insight embedding
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            include_insights: true,
            include_relations: false,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    assert!(!candidates.insights.is_empty());
    assert_eq!(
        candidates.insights[0].content,
        "Error handling pattern detected"
    );
}

#[test]
fn test_context_includes_relations() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_ids = record_experiences(&db, cid, &[10, 20]);

    // Store a relation between the two experiences
    let rel_id = db
        .store_relation(NewExperienceRelation {
            source_id: exp_ids[0],
            target_id: exp_ids[1],
            relation_type: RelationType::Supports,
            strength: 0.8,
            metadata: None,
        })
        .unwrap();

    let query = make_embedding(10); // Similar to exp_ids[0]
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            include_insights: false,
            include_relations: true,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    // The relation should appear because exp_ids[0] and/or exp_ids[1] are in results
    assert!(
        !candidates.relations.is_empty(),
        "Expected relations for returned experiences"
    );
    assert!(candidates.relations.iter().any(|r| r.id == rel_id));
}

#[test]
fn test_context_includes_active_agents() {
    let (db, cid, _dir) = open_db_with_collective();
    record_experiences(&db, cid, &[1]);

    // Register an active agent
    db.register_activity(NewActivity {
        agent_id: "claude-opus".to_string(),
        collective_id: cid,
        current_task: Some("Reviewing code".to_string()),
        context_summary: None,
    })
    .unwrap();

    let query = make_embedding(1);
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            include_insights: false,
            include_relations: false,
            include_active_agents: true,
            ..ContextRequest::default()
        })
        .unwrap();

    assert_eq!(candidates.active_agents.len(), 1);
    assert_eq!(candidates.active_agents[0].agent_id, "claude-opus");
}

// ============================================================================
// Flag Behavior
// ============================================================================

#[test]
fn test_context_respects_flags() {
    let (db, cid, _dir) = open_db_with_collective();
    let exp_ids = record_experiences(&db, cid, &[10, 20]);

    // Set up ALL primitives: insight, relation, activity
    db.store_insight(NewDerivedInsight {
        collective_id: cid,
        content: "Test insight".to_string(),
        embedding: Some(make_embedding(15)),
        source_experience_ids: exp_ids.clone(),
        insight_type: InsightType::Synthesis,
        confidence: 0.7,
        domain: vec![],
    })
    .unwrap();

    db.store_relation(NewExperienceRelation {
        source_id: exp_ids[0],
        target_id: exp_ids[1],
        relation_type: RelationType::Elaborates,
        strength: 0.6,
        metadata: None,
    })
    .unwrap();

    db.register_activity(NewActivity {
        agent_id: "test-agent".to_string(),
        collective_id: cid,
        current_task: None,
        context_summary: None,
    })
    .unwrap();

    // Request with ALL include flags set to false
    let query = make_embedding(10);
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            include_insights: false,
            include_relations: false,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    // Similar and recent should still be populated
    assert!(!candidates.similar_experiences.is_empty());
    assert!(!candidates.recent_experiences.is_empty());

    // Flagged-off sections should be empty
    assert!(
        candidates.insights.is_empty(),
        "Insights should be excluded when flag is false"
    );
    assert!(
        candidates.relations.is_empty(),
        "Relations should be excluded when flag is false"
    );
    assert!(
        candidates.active_agents.is_empty(),
        "Active agents should be excluded when flag is false"
    );
}

// ============================================================================
// Filtering
// ============================================================================

#[test]
fn test_context_respects_filters() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record experiences with different domains and importance
    for i in 0..5 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Rust experience {}", i),
            embedding: Some(make_embedding(i)),
            domain: vec!["rust".to_string()],
            importance: 0.8,
            ..Default::default()
        })
        .unwrap();
    }
    for i in 10..15 {
        db.record_experience(NewExperience {
            collective_id: cid,
            content: format!("Python experience {}", i),
            embedding: Some(make_embedding(i)),
            domain: vec!["python".to_string()],
            importance: 0.3,
            ..Default::default()
        })
        .unwrap();
    }

    let query = make_embedding(2);
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            filter: SearchFilter {
                domains: Some(vec!["rust".to_string()]),
                min_importance: Some(0.5),
                ..SearchFilter::default()
            },
            include_insights: false,
            include_relations: false,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    // All similar results should be rust domain with importance >= 0.5
    for result in &candidates.similar_experiences {
        assert!(
            result.experience.domain.contains(&"rust".to_string()),
            "Expected rust domain, got {:?}",
            result.experience.domain
        );
        assert!(
            result.experience.importance >= 0.5,
            "Expected importance >= 0.5, got {}",
            result.experience.importance
        );
    }

    // All recent results should also respect filters
    for exp in &candidates.recent_experiences {
        assert!(exp.domain.contains(&"rust".to_string()));
        assert!(exp.importance >= 0.5);
    }
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_context_empty_collective() {
    let (db, cid, _dir) = open_db_with_collective();

    // No experiences, no insights, no activities — just an empty collective
    let query = make_embedding(0);
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            ..ContextRequest::default()
        })
        .unwrap();

    assert!(candidates.similar_experiences.is_empty());
    assert!(candidates.recent_experiences.is_empty());
    assert!(candidates.insights.is_empty());
    assert!(candidates.relations.is_empty());
    assert!(candidates.active_agents.is_empty());
}

// ============================================================================
// Validation
// ============================================================================

#[test]
fn test_context_validation_dimension_mismatch() {
    let (db, cid, _dir) = open_db_with_collective();

    // Wrong dimension (3 instead of 384)
    let result = db.get_context_candidates(ContextRequest {
        collective_id: cid,
        query_embedding: vec![0.1, 0.2, 0.3],
        ..ContextRequest::default()
    });

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("dimension"),
        "Error should mention dimension: {}",
        err
    );
}

#[test]
fn test_context_validation_invalid_limits() {
    let (db, cid, _dir) = open_db_with_collective();
    let query = make_embedding(0);

    // max_similar = 0
    let result = db.get_context_candidates(ContextRequest {
        collective_id: cid,
        query_embedding: query.clone(),
        max_similar: 0,
        ..ContextRequest::default()
    });
    assert!(result.is_err());

    // max_similar = 1001
    let result = db.get_context_candidates(ContextRequest {
        collective_id: cid,
        query_embedding: query.clone(),
        max_similar: 1001,
        ..ContextRequest::default()
    });
    assert!(result.is_err());

    // max_recent = 0
    let result = db.get_context_candidates(ContextRequest {
        collective_id: cid,
        query_embedding: query.clone(),
        max_recent: 0,
        ..ContextRequest::default()
    });
    assert!(result.is_err());

    // max_recent = 1001
    let result = db.get_context_candidates(ContextRequest {
        collective_id: cid,
        query_embedding: query,
        max_recent: 1001,
        ..ContextRequest::default()
    });
    assert!(result.is_err());
}

#[test]
fn test_context_validation_nonexistent_collective() {
    let (db, _dir) = open_db();

    let result = db.get_context_candidates(ContextRequest {
        collective_id: CollectiveId::new(), // Random ID, doesn't exist
        query_embedding: make_embedding(0),
        ..ContextRequest::default()
    });

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found") || err.contains("Collective"),
        "Error should indicate collective not found: {}",
        err
    );
}

// ============================================================================
// Relation Deduplication
// ============================================================================

#[test]
fn test_context_relations_deduplicated() {
    let (db, cid, _dir) = open_db_with_collective();

    // Record 3 experiences with close embeddings so they all appear in results
    let exp_ids = record_experiences(&db, cid, &[1, 2, 3]);

    // Create relations: A→B and B→C
    let rel_ab = db
        .store_relation(NewExperienceRelation {
            source_id: exp_ids[0],
            target_id: exp_ids[1],
            relation_type: RelationType::Supports,
            strength: 0.9,
            metadata: None,
        })
        .unwrap();

    let rel_bc = db
        .store_relation(NewExperienceRelation {
            source_id: exp_ids[1],
            target_id: exp_ids[2],
            relation_type: RelationType::Elaborates,
            strength: 0.7,
            metadata: None,
        })
        .unwrap();

    let query = make_embedding(2); // Similar to seed=2 (middle experience)
    let candidates = db
        .get_context_candidates(ContextRequest {
            collective_id: cid,
            query_embedding: query,
            max_similar: 10,
            max_recent: 10,
            include_relations: true,
            include_insights: false,
            include_active_agents: false,
            ..ContextRequest::default()
        })
        .unwrap();

    // Check that relations are present (the exact set depends on which
    // experiences are returned by HNSW, but we know at least some relations exist)
    // The key property: no duplicate RelationIds
    let all_ids: Vec<_> = candidates.relations.iter().map(|r| r.id).collect();
    let unique_ids: std::collections::HashSet<_> = all_ids.iter().collect();
    assert_eq!(
        unique_ids.len(),
        all_ids.len(),
        "Relations should be deduplicated by RelationId"
    );

    // Verify our known relations are present if their experiences were returned
    let returned_exp_ids: Vec<_> = candidates
        .similar_experiences
        .iter()
        .map(|r| r.experience.id)
        .chain(candidates.recent_experiences.iter().map(|e| e.id))
        .collect();

    if returned_exp_ids.contains(&exp_ids[0]) || returned_exp_ids.contains(&exp_ids[1]) {
        assert!(
            candidates.relations.iter().any(|r| r.id == rel_ab),
            "Expected rel_ab when A or B is in results"
        );
    }
    if returned_exp_ids.contains(&exp_ids[1]) || returned_exp_ids.contains(&exp_ids[2]) {
        assert!(
            candidates.relations.iter().any(|r| r.id == rel_bc),
            "Expected rel_bc when B or C is in results"
        );
    }
}
