//! Property-based tests for PulseDB invariants (E5-S03, Ticket #20).
//!
//! Uses proptest to verify properties that must hold for ANY valid input,
//! not just hand-picked examples. Each property is tested with 50 random
//! cases (configurable via PROPTEST_CASES env var).
//!
//! Properties tested:
//! 1. search_similar never returns more than k results
//! 2. importance stays in [0.0, 1.0] after store/retrieve
//! 3. confidence stays in [0.0, 1.0] after store/retrieve
//! 4. experience content round-trips without corruption
//! 5. collective isolation — searches never leak across collectives
//! 6. archived experiences never appear in search results
//! 7. relations are directional — A→B does not create B→A

use proptest::prelude::*;
use pulsedb::{
    CollectiveId, Config, NewExperience, NewExperienceRelation, PulseDB, RelationDirection,
    RelationType,
};
use tempfile::tempdir;

/// Default embedding dimension for tests (D384).
const DIM: usize = 384;

/// Generates a deterministic embedding from a seed.
///
/// Uses a hash-based pseudo-random generator to produce well-separated vectors
/// in the 384-dimensional space. Reused from tests/search_similar.rs.
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

/// Reduced case count: each case opens a real redb database on disk.
/// 50 cases balances thoroughness with speed (~2-3s per property).
fn config() -> ProptestConfig {
    ProptestConfig::with_cases(50)
}

// ============================================================================
// Property 1: search_similar never returns more than k results
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_search_never_returns_more_than_k(k in 1usize..50) {
        let (db, cid, _dir) = open_db_with_collective();

        // Insert 20 experiences with distinct, well-separated embeddings
        let seeds: Vec<u64> = (0..20).collect();
        for &seed in &seeds {
            db.record_experience(NewExperience {
                collective_id: cid,
                content: format!("Experience seed={seed}"),
                embedding: Some(make_embedding(seed)),
                ..Default::default()
            })
            .unwrap();
        }

        let query = make_embedding(100); // query different from all seeds
        let results = db.search_similar(cid, &query, k).unwrap();

        prop_assert!(
            results.len() <= k,
            "search_similar returned {} results for k={}, expected at most k",
            results.len(),
            k
        );
    }
}

// ============================================================================
// Property 2: importance stays in [0.0, 1.0] after store/retrieve
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_importance_preserved_in_range(importance in 0.0f32..=1.0) {
        let (db, cid, _dir) = open_db_with_collective();

        let id = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: "Importance range test".to_string(),
                embedding: Some(dummy_embedding()),
                importance,
                ..Default::default()
            })
            .unwrap();

        let exp = db.get_experience(id).unwrap().unwrap();

        prop_assert!(
            (0.0..=1.0).contains(&exp.importance),
            "importance {} out of range after store/retrieve",
            exp.importance
        );
        prop_assert!(
            (exp.importance - importance).abs() < f32::EPSILON,
            "importance changed: stored {} but got {}",
            importance,
            exp.importance
        );
    }
}

// ============================================================================
// Property 3: confidence stays in [0.0, 1.0] after store/retrieve
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_confidence_preserved_in_range(confidence in 0.0f32..=1.0) {
        let (db, cid, _dir) = open_db_with_collective();

        let id = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: "Confidence range test".to_string(),
                embedding: Some(dummy_embedding()),
                confidence,
                ..Default::default()
            })
            .unwrap();

        let exp = db.get_experience(id).unwrap().unwrap();

        prop_assert!(
            (0.0..=1.0).contains(&exp.confidence),
            "confidence {} out of range after store/retrieve",
            exp.confidence
        );
        prop_assert!(
            (exp.confidence - confidence).abs() < f32::EPSILON,
            "confidence changed: stored {} but got {}",
            confidence,
            exp.confidence
        );
    }
}

// ============================================================================
// Property 4: experience content round-trips without corruption
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_content_roundtrips(content in "[a-zA-Z0-9 ]{1,200}") {
        let (db, cid, _dir) = open_db_with_collective();

        let id = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: content.clone(),
                embedding: Some(dummy_embedding()),
                ..Default::default()
            })
            .unwrap();

        let exp = db.get_experience(id).unwrap().unwrap();

        prop_assert_eq!(
            &exp.content,
            &content,
            "content corrupted during store/retrieve"
        );
    }
}

// ============================================================================
// Property 5: collective isolation — searches never leak across collectives
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_collective_isolation(n_a in 1usize..6, n_b in 1usize..6) {
        let (db, _dir) = open_db();
        let cid_a = db.create_collective("collective-a").unwrap();
        let cid_b = db.create_collective("collective-b").unwrap();

        // Insert n_a experiences in collective A
        for i in 0..n_a {
            db.record_experience(NewExperience {
                collective_id: cid_a,
                content: format!("A-experience-{i}"),
                embedding: Some(make_embedding(i as u64)),
                ..Default::default()
            })
            .unwrap();
        }

        // Insert n_b experiences in collective B (offset seeds to avoid collisions)
        for i in 0..n_b {
            db.record_experience(NewExperience {
                collective_id: cid_b,
                content: format!("B-experience-{i}"),
                embedding: Some(make_embedding((i + 100) as u64)),
                ..Default::default()
            })
            .unwrap();
        }

        // Search in collective A — should only find A's experiences
        let query = make_embedding(0);
        let results = db.search_similar(cid_a, &query, 50).unwrap();

        for result in &results {
            prop_assert_eq!(
                result.experience.collective_id,
                cid_a,
                "search in collective A returned experience from collective B"
            );
        }

        // Search in collective B — should only find B's experiences
        let results = db.search_similar(cid_b, &query, 50).unwrap();

        for result in &results {
            prop_assert_eq!(
                result.experience.collective_id,
                cid_b,
                "search in collective B returned experience from collective A"
            );
        }
    }
}

// ============================================================================
// Property 6: archived experiences never appear in search results
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_archived_excluded_from_search(
        archive_mask in proptest::collection::vec(proptest::bool::ANY, 10)
    ) {
        let (db, cid, _dir) = open_db_with_collective();

        // Insert 10 experiences with distinct embeddings
        let mut ids = Vec::new();
        for seed in 0u64..10 {
            let id = db
                .record_experience(NewExperience {
                    collective_id: cid,
                    content: format!("Experience seed={seed}"),
                    embedding: Some(make_embedding(seed)),
                    ..Default::default()
                })
                .unwrap();
            ids.push(id);
        }

        // Archive experiences according to the random mask
        let mut archived_ids = std::collections::HashSet::new();
        for (i, &should_archive) in archive_mask.iter().enumerate() {
            if should_archive {
                db.archive_experience(ids[i]).unwrap();
                archived_ids.insert(ids[i]);
            }
        }

        // Search should never return archived experiences
        let query = make_embedding(5);
        let results = db.search_similar(cid, &query, 20).unwrap();

        for result in &results {
            prop_assert!(
                !archived_ids.contains(&result.experience.id),
                "search returned archived experience {:?}",
                result.experience.id
            );
        }
    }
}

// ============================================================================
// Property 7: relations are directional — storing A→B does NOT create B→A
// ============================================================================

proptest! {
    #![proptest_config(config())]

    #[test]
    fn prop_relations_are_directional(
        relation_type_idx in 0usize..6,
        strength in 0.0f32..=1.0
    ) {
        let (db, cid, _dir) = open_db_with_collective();

        // Create two experiences
        let exp_a = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: "Source experience A".to_string(),
                embedding: Some(make_embedding(0)),
                ..Default::default()
            })
            .unwrap();

        let exp_b = db
            .record_experience(NewExperience {
                collective_id: cid,
                content: "Target experience B".to_string(),
                embedding: Some(make_embedding(1)),
                ..Default::default()
            })
            .unwrap();

        // Pick a relation type from the index
        let relation_types = [
            RelationType::Supports,
            RelationType::Contradicts,
            RelationType::Elaborates,
            RelationType::Supersedes,
            RelationType::Implies,
            RelationType::RelatedTo,
        ];
        let relation_type = relation_types[relation_type_idx];

        // Store A → B relation
        db.store_relation(NewExperienceRelation {
            source_id: exp_a,
            target_id: exp_b,
            relation_type,
            strength,
            metadata: None,
        })
        .unwrap();

        // Outgoing from A should find B
        let outgoing_a = db
            .get_related_experiences(exp_a, RelationDirection::Outgoing)
            .unwrap();
        prop_assert!(
            outgoing_a.iter().any(|(exp, _)| exp.id == exp_b),
            "A→B relation not found in A's outgoing"
        );

        // Outgoing from B should be empty (relation is A→B, not B→A)
        let outgoing_b = db
            .get_related_experiences(exp_b, RelationDirection::Outgoing)
            .unwrap();
        prop_assert!(
            outgoing_b.is_empty(),
            "B has outgoing relations but shouldn't — relation was A→B, not B→A"
        );

        // Incoming to B should find A
        let incoming_b = db
            .get_related_experiences(exp_b, RelationDirection::Incoming)
            .unwrap();
        prop_assert!(
            incoming_b.iter().any(|(exp, _)| exp.id == exp_a),
            "A→B relation not found in B's incoming"
        );

        // Incoming to A should be empty
        let incoming_a = db
            .get_related_experiences(exp_a, RelationDirection::Incoming)
            .unwrap();
        prop_assert!(
            incoming_a.is_empty(),
            "A has incoming relations but shouldn't — relation was A→B"
        );
    }
}
