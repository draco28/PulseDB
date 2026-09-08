//! Integration tests for Phase 3: Sync Engine.
//!
//! Tests two real PulseDB instances syncing via InMemorySyncTransport.
//! Covers push, pull, bidirectional sync, conflict resolution, echo
//! prevention, incremental sync, and SyncManager lifecycle.

#![cfg(feature = "sync")]

mod common;

use std::sync::Arc;

use common::{copy_fixture, fixtures_dir, sync_both_ways};
use pulsedb::sync::config::{ConflictResolution, SyncConfig, SyncDirection};
use pulsedb::sync::manager::SyncManager;
use pulsedb::sync::transport_mem::InMemorySyncTransport;
use pulsedb::sync::SyncStatus;
use pulsedb::{
    CollectiveId, Config, ExperienceId, ExperienceUpdate, InsightType, NewDerivedInsight,
    NewExperience, NewExperienceRelation, PulseDB, RelationType,
};
use tempfile::tempdir;

// ============================================================================
// Helpers
// ============================================================================

fn open_db() -> (Arc<PulseDB>, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db = Arc::new(PulseDB::open(dir.path().join("test.db"), Config::default()).unwrap());
    (db, dir)
}

fn minimal_exp(cid: CollectiveId) -> NewExperience {
    NewExperience {
        collective_id: cid,
        content: format!("experience-{}", uuid::Uuid::now_v7()),
        embedding: Some(vec![0.1f32; 384]),
        ..Default::default()
    }
}

fn sync_config() -> SyncConfig {
    SyncConfig {
        direction: SyncDirection::Bidirectional,
        batch_size: 500,
        ..Default::default()
    }
}

/// Create two PulseDB instances with paired transports and SyncManagers.
fn setup_sync_pair() -> SyncPair {
    let (db_a, dir_a) = open_db();
    let (db_b, dir_b) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();

    let manager_a = SyncManager::new(Arc::clone(&db_a), Box::new(transport_a), sync_config());
    let manager_b = SyncManager::new(Arc::clone(&db_b), Box::new(transport_b), sync_config());

    SyncPair {
        db_a,
        db_b,
        manager_a,
        manager_b,
        _dir_a: dir_a,
        _dir_b: dir_b,
    }
}

struct SyncPair {
    db_a: Arc<PulseDB>,
    db_b: Arc<PulseDB>,
    manager_a: SyncManager,
    manager_b: SyncManager,
    _dir_a: tempfile::TempDir,
    _dir_b: tempfile::TempDir,
}

// ============================================================================
// Basic push + pull
// ============================================================================

#[tokio::test]
async fn test_basic_experience_sync() {
    let mut pair = setup_sync_pair();

    // Create collective + experience on A
    let cid = pair.db_a.create_collective("test").unwrap();
    let exp_id = pair.db_a.record_experience(minimal_exp(cid)).unwrap();

    // Sync A → shared buffer
    pair.manager_a.sync_once().await.unwrap();

    // Create same collective on B (needed for HNSW indexes)
    // In real usage, collective sync handles this. Here we create it manually
    // since B needs the collective before it can receive experiences.
    pair.db_b.create_collective("test").unwrap();

    // Sync B ← shared buffer
    pair.manager_b.sync_once().await.unwrap();

    // Verify B has the experience
    let exp = pair.db_b.get_experience(exp_id).unwrap();
    assert!(exp.is_some(), "Experience should have synced to DB-B");
    assert!(exp.unwrap().content.starts_with("experience-"));
}

#[tokio::test]
async fn test_collective_sync() {
    let mut pair = setup_sync_pair();

    // Create collective on A
    let cid = pair.db_a.create_collective("synced-collective").unwrap();

    // Sync A → buffer → B
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();

    // B should have the collective
    let collective = pair.db_b.get_collective(cid).unwrap();
    assert!(collective.is_some(), "Collective should sync to DB-B");
    assert_eq!(collective.unwrap().name, "synced-collective");

    // B should be able to record experiences in the synced collective
    let exp_id = pair.db_b.record_experience(minimal_exp(cid)).unwrap();
    assert!(pair.db_b.get_experience(exp_id).unwrap().is_some());
}

#[tokio::test]
async fn test_experience_with_collective_sync() {
    let mut pair = setup_sync_pair();

    // Create collective + experience on A
    let cid = pair.db_a.create_collective("proj").unwrap();
    let exp_id = pair.db_a.record_experience(minimal_exp(cid)).unwrap();

    // Sync A → buffer
    pair.manager_a.sync_once().await.unwrap();

    // Sync B ← buffer (collective + experience arrive together)
    pair.manager_b.sync_once().await.unwrap();

    // B should have both
    assert!(pair.db_b.get_collective(cid).unwrap().is_some());
    assert!(pair.db_b.get_experience(exp_id).unwrap().is_some());
}

#[tokio::test]
async fn test_relation_sync() {
    let mut pair = setup_sync_pair();

    let cid = pair.db_a.create_collective("rel-test").unwrap();
    let exp1 = pair.db_a.record_experience(minimal_exp(cid)).unwrap();
    let exp2 = pair.db_a.record_experience(minimal_exp(cid)).unwrap();
    let rel_id = pair
        .db_a
        .store_relation(NewExperienceRelation {
            source_id: exp1,
            target_id: exp2,
            relation_type: RelationType::Supports,
            strength: 0.9,
            metadata: None,
        })
        .unwrap();

    // Sync A → B
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();

    // B should have the relation
    let rel = pair.db_b.get_relation(rel_id).unwrap();
    assert!(rel.is_some(), "Relation should sync to DB-B");
    let rel = rel.unwrap();
    assert_eq!(rel.source_id, exp1);
    assert_eq!(rel.target_id, exp2);
}

#[tokio::test]
async fn test_insight_sync() {
    let mut pair = setup_sync_pair();

    let cid = pair.db_a.create_collective("insight-test").unwrap();
    let exp_id = pair.db_a.record_experience(minimal_exp(cid)).unwrap();
    let insight_id = pair
        .db_a
        .store_insight(NewDerivedInsight {
            collective_id: cid,
            content: "synced insight".to_string(),
            embedding: Some(vec![0.2f32; 384]),
            source_experience_ids: vec![exp_id],
            insight_type: InsightType::Pattern,
            confidence: 0.8,
            domain: vec!["test".to_string()],
        })
        .unwrap();

    // Sync A → B
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();

    let insight = pair.db_b.get_insight(insight_id).unwrap();
    assert!(insight.is_some(), "Insight should sync to DB-B");
    assert_eq!(insight.unwrap().content, "synced insight");
}

// ============================================================================
// Delete sync
// ============================================================================

#[tokio::test]
async fn test_experience_delete_sync() {
    let mut pair = setup_sync_pair();

    let cid = pair.db_a.create_collective("del-test").unwrap();
    let exp_id = pair.db_a.record_experience(minimal_exp(cid)).unwrap();

    // Sync creation
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();
    assert!(pair.db_b.get_experience(exp_id).unwrap().is_some());

    // Delete on A
    pair.db_a.delete_experience(exp_id).unwrap();

    // Sync deletion
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();

    // B should no longer have it
    assert!(
        pair.db_b.get_experience(exp_id).unwrap().is_none(),
        "Deleted experience should be gone on DB-B"
    );
}

// ============================================================================
// Update sync
// ============================================================================

#[tokio::test]
async fn test_experience_update_sync() {
    let mut pair = setup_sync_pair();

    let cid = pair.db_a.create_collective("upd-test").unwrap();
    let exp_id = pair.db_a.record_experience(minimal_exp(cid)).unwrap();

    // Sync creation
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();

    // Update on A
    pair.db_a
        .update_experience(
            exp_id,
            ExperienceUpdate {
                importance: Some(0.99),
                ..Default::default()
            },
        )
        .unwrap();

    // Sync update
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();

    let exp = pair.db_b.get_experience(exp_id).unwrap().unwrap();
    assert!(
        (exp.importance - 0.99).abs() < f32::EPSILON,
        "Updated importance should sync"
    );
}

// ============================================================================
// Incremental sync
// ============================================================================

#[tokio::test]
async fn test_incremental_sync() {
    let mut pair = setup_sync_pair();

    let cid = pair.db_a.create_collective("inc-test").unwrap();

    // First batch
    let id1 = pair.db_a.record_experience(minimal_exp(cid)).unwrap();
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();
    assert!(pair.db_b.get_experience(id1).unwrap().is_some());

    // Second batch (only new changes should sync)
    let id2 = pair.db_a.record_experience(minimal_exp(cid)).unwrap();
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();
    assert!(pair.db_b.get_experience(id2).unwrap().is_some());

    // Third sync with no new changes
    let status = pair.manager_a.sync_once().await.unwrap();
    assert_eq!(status, SyncStatus::Idle);
}

// ============================================================================
// Echo prevention
// ============================================================================

#[tokio::test]
async fn test_echo_prevention() {
    let mut pair = setup_sync_pair();

    let cid = pair.db_a.create_collective("echo-test").unwrap();
    let exp_id = pair.db_a.record_experience(minimal_exp(cid)).unwrap();

    // Sync A → B
    pair.manager_a.sync_once().await.unwrap();
    pair.manager_b.sync_once().await.unwrap();
    assert!(pair.db_b.get_experience(exp_id).unwrap().is_some());

    // B syncs back to shared buffer — the synced experience should NOT
    // be pushed back (echo prevention)
    let seq_before = pair.db_b.get_current_sequence().unwrap();
    pair.manager_b.sync_once().await.unwrap();
    assert_eq!(pair.db_b.get_current_sequence().unwrap(), seq_before);

    // A syncs again — should have NO new changes from B
    pair.manager_a.sync_once().await.unwrap();

    // The experience on A should still be the original (not duplicated)
    let exp = pair.db_a.get_experience(exp_id).unwrap().unwrap();
    assert_eq!(exp.applications(), 0); // Not modified
}

// ============================================================================
// Conflict resolution
// ============================================================================

#[tokio::test]
async fn test_conflict_resolution_server_wins() {
    let (db_a, _dir_a) = open_db();
    let (db_b, _dir_b) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();

    let config = SyncConfig {
        conflict_resolution: ConflictResolution::ServerWins,
        ..sync_config()
    };

    let mut manager_a = SyncManager::new(Arc::clone(&db_a), Box::new(transport_a), config.clone());
    let mut manager_b = SyncManager::new(Arc::clone(&db_b), Box::new(transport_b), config);

    // Create on A, sync to B
    let cid = db_a.create_collective("conflict").unwrap();
    let exp_id = db_a.record_experience(minimal_exp(cid)).unwrap();

    manager_a.sync_once().await.unwrap();
    manager_b.sync_once().await.unwrap();

    // Update on A (remote/server)
    db_a.update_experience(
        exp_id,
        ExperienceUpdate {
            importance: Some(0.1),
            ..Default::default()
        },
    )
    .unwrap();

    // Sync update A → B (ServerWins: remote always wins)
    manager_a.sync_once().await.unwrap();
    manager_b.sync_once().await.unwrap();

    let exp_b = db_b.get_experience(exp_id).unwrap().unwrap();
    assert!(
        (exp_b.importance - 0.1).abs() < f32::EPSILON,
        "ServerWins: remote update should be applied"
    );
}

// ============================================================================
// Bidirectional sync
// ============================================================================

#[tokio::test]
async fn test_bidirectional_sync() {
    // Bidirectional sync uses two separate transport pairs:
    // A→B transport and B→A transport. The InMemorySyncTransport
    // shares a single buffer, so both directions need separate pairs.
    let (db_a, _dir_a) = open_db();
    let (db_b, _dir_b) = open_db();

    // A→B direction
    let (transport_a_push, transport_b_pull) = InMemorySyncTransport::new_pair();
    // B→A direction
    let (transport_b_push, transport_a_pull) = InMemorySyncTransport::new_pair();

    let config_a_push = SyncConfig {
        direction: SyncDirection::PushOnly,
        ..sync_config()
    };
    let config_b_pull = SyncConfig {
        direction: SyncDirection::PullOnly,
        ..sync_config()
    };
    let config_b_push = SyncConfig {
        direction: SyncDirection::PushOnly,
        ..sync_config()
    };
    let config_a_pull = SyncConfig {
        direction: SyncDirection::PullOnly,
        ..sync_config()
    };

    let mut mgr_a_push =
        SyncManager::new(Arc::clone(&db_a), Box::new(transport_a_push), config_a_push);
    let mut mgr_b_pull =
        SyncManager::new(Arc::clone(&db_b), Box::new(transport_b_pull), config_b_pull);
    let mut mgr_b_push =
        SyncManager::new(Arc::clone(&db_b), Box::new(transport_b_push), config_b_push);
    let mut mgr_a_pull =
        SyncManager::new(Arc::clone(&db_a), Box::new(transport_a_pull), config_a_pull);

    // Create collective on A, push to B
    let cid = db_a.create_collective("bidi").unwrap();
    mgr_a_push.sync_once().await.unwrap();
    mgr_b_pull.sync_once().await.unwrap();

    // Create experiences on both sides
    let id_a = db_a.record_experience(minimal_exp(cid)).unwrap();
    let id_b = db_b.record_experience(minimal_exp(cid)).unwrap();

    // Push A→B, Pull B←A
    mgr_a_push.sync_once().await.unwrap();
    mgr_b_pull.sync_once().await.unwrap();

    // Push B→A, Pull A←B
    mgr_b_push.sync_once().await.unwrap();
    mgr_a_pull.sync_once().await.unwrap();

    // Both should have both experiences
    assert!(db_a.get_experience(id_a).unwrap().is_some());
    assert!(
        db_a.get_experience(id_b).unwrap().is_some(),
        "A should have B's experience"
    );
    assert!(db_b.get_experience(id_a).unwrap().is_some());
    assert!(db_b.get_experience(id_b).unwrap().is_some());
}

#[tokio::test]
async fn test_bidirectional_reinforcement_gcounter_converges_exact_total() {
    let (db_a, _dir_a) = open_db();
    let (db_b, _dir_b) = open_db();

    let (transport_a_push, transport_b_pull) = InMemorySyncTransport::new_pair();
    let (transport_b_push, transport_a_pull) = InMemorySyncTransport::new_pair();

    let mut mgr_a_push = SyncManager::new(
        Arc::clone(&db_a),
        Box::new(transport_a_push),
        SyncConfig {
            direction: SyncDirection::PushOnly,
            ..sync_config()
        },
    );
    let mut mgr_b_pull = SyncManager::new(
        Arc::clone(&db_b),
        Box::new(transport_b_pull),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    );
    let mut mgr_b_push = SyncManager::new(
        Arc::clone(&db_b),
        Box::new(transport_b_push),
        SyncConfig {
            direction: SyncDirection::PushOnly,
            ..sync_config()
        },
    );
    let mut mgr_a_pull = SyncManager::new(
        Arc::clone(&db_a),
        Box::new(transport_a_pull),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    );

    let cid = db_a.create_collective("reinforce-gcounter").unwrap();
    let exp_id = db_a.record_experience(minimal_exp(cid)).unwrap();
    mgr_a_push.sync_once().await.unwrap();
    mgr_b_pull.sync_once().await.unwrap();

    db_a.reinforce_experience(exp_id).unwrap();
    db_b.reinforce_experience(exp_id).unwrap();
    db_b.reinforce_experience(exp_id).unwrap();

    mgr_a_push.sync_once().await.unwrap();
    mgr_b_pull.sync_once().await.unwrap();
    mgr_b_push.sync_once().await.unwrap();
    mgr_a_pull.sync_once().await.unwrap();

    let exp_a = db_a.get_experience(exp_id).unwrap().unwrap();
    let exp_b = db_b.get_experience(exp_id).unwrap().unwrap();
    assert_eq!(exp_a.applications(), 3);
    assert_eq!(exp_b.applications(), 3);
    assert_eq!(exp_a.applications, exp_b.applications);
}

/// The reserved bucket the v2→v3 migration files a store's scalar
/// `applications` count under. Mirrors the private
/// `legacy_applications_instance_id()` in `src/storage/redb.rs`: a fixed,
/// non-UUIDv7 key that every migrated store shares, so two independently
/// migrated copies of one v0.4.0 store merge their legacy counts under
/// per-key max (one bucket) instead of summing them (two buckets).
fn legacy_sentinel() -> pulsedb::InstanceId {
    pulsedb::InstanceId::from_bytes(*b"PULSEDB_LEGACY__")
}

/// Issue #11: the sentinel-merge proof runs on two REAL v0.4.0 stores
/// (schema v2, scalar `applications`), each migrated through the genuine
/// v2→v3 path on open, then synced bidirectionally. Every experience collides
/// on create, and the merged total must equal the legacy count — not twice it.
#[tokio::test]
async fn test_create_collision_sentinel_merge_does_not_double_count() {
    use std::collections::BTreeMap;

    // Two independent restores of the same real v0.4.0 store, each migrated
    // by its own open (the checked-in blob is never opened directly).
    let (_tmp_a, path_a) = copy_fixture("real-v0.4.0.redb");
    let (_tmp_b, path_b) = copy_fixture("real-v0.4.0.redb");
    let db_a = Arc::new(PulseDB::open(&path_a, Config::default()).unwrap());
    let db_b = Arc::new(PulseDB::open(&path_b, Config::default()).unwrap());

    // v0.4.0 persisted no identity, so each copy minted its own on migration:
    // the two stores are distinct sync actors, exactly as two restores are.
    let id_a = db_a.instance_id();
    let id_b = db_b.instance_id();
    assert_ne!(id_a, id_b, "each migrated copy mints its own identity");

    // Oracle: the manifest's captured pre-migration scalar counts.
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures_dir().join("real-v0.4.0.manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest["schema_version"], 2,
        "fixture must be a schema-v2 store"
    );
    let legacy: Vec<(ExperienceId, u32)> = manifest["experiences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let id = ExperienceId(uuid::Uuid::parse_str(e["id"].as_str().unwrap()).unwrap());
            let count = u32::try_from(e["applications"].as_u64().unwrap()).unwrap();
            (id, count)
        })
        .collect();
    assert!(!legacy.is_empty(), "fixture carries experiences");

    // Post-migration, pre-sync: the scalar count landed in the sentinel bucket
    // on both copies, and nowhere else.
    for (id, count) in &legacy {
        for db in [&db_a, &db_b] {
            let exp = db.get_experience(*id).unwrap().unwrap();
            assert_eq!(
                exp.applications,
                BTreeMap::from([(legacy_sentinel(), *count)]),
                "experience {id}: v2 scalar must migrate into exactly the sentinel bucket"
            );
        }
    }

    // Sync both ways. Every collective and experience already exists on the
    // other side, so each ExperienceCreated is a create collision that goes
    // through the G-counter merge — the sentinel must merge by key, not add.
    sync_both_ways(&db_a, &db_b).await;

    for (id, count) in &legacy {
        for db in [&db_a, &db_b] {
            let exp = db.get_experience(*id).unwrap().unwrap();
            assert_eq!(
                exp.applications(),
                *count,
                "experience {id}: merged total must equal the legacy count, not twice it"
            );
            assert_eq!(
                exp.applications,
                BTreeMap::from([(legacy_sentinel(), *count)]),
                "experience {id}: still exactly one sentinel bucket after sync"
            );
        }
    }

    // Fresh reinforcements on each migrated copy land in each copy's own
    // minted bucket and sum exactly on top of the sentinel. This part bites
    // whatever the fixture's scalar values are: a sentinel renamed, or minted
    // per store, would surface here as an extra bucket.
    let (probe, probe_legacy) = legacy[0];
    for _ in 0..2 {
        db_a.reinforce_experience(probe).unwrap();
    }
    for _ in 0..3 {
        db_b.reinforce_experience(probe).unwrap();
    }
    sync_both_ways(&db_a, &db_b).await;

    let expected = BTreeMap::from([(legacy_sentinel(), probe_legacy), (id_a, 2), (id_b, 3)]);
    for db in [&db_a, &db_b] {
        let exp = db.get_experience(probe).unwrap().unwrap();
        assert_eq!(
            exp.applications, expected,
            "sentinel + one bucket per migrated copy"
        );
        assert_eq!(exp.applications(), probe_legacy + 5);
    }
}

// ============================================================================
// Initial sync
// ============================================================================

#[tokio::test]
async fn test_initial_sync_catchup() {
    let (db_a, _dir_a) = open_db();
    let (db_b, _dir_b) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();

    let config = SyncConfig {
        batch_size: 5, // Small batches to test pagination
        ..sync_config()
    };

    let mut manager_a = SyncManager::new(Arc::clone(&db_a), Box::new(transport_a), config.clone());
    let mut manager_b = SyncManager::new(Arc::clone(&db_b), Box::new(transport_b), config);

    // Create a bunch of data on A
    let cid = db_a.create_collective("catchup").unwrap();
    let mut exp_ids = Vec::new();
    for _ in 0..12 {
        exp_ids.push(db_a.record_experience(minimal_exp(cid)).unwrap());
    }

    // Push all from A. One cycle pushes at most `batch_size` changes — that is
    // what `batch_size` means on the push path — so drain the 13 WAL events
    // (the collective plus twelve experiences) in ceil(13 / 5) cycles.
    for _ in 0..3 {
        manager_a.sync_once().await.unwrap();
    }

    // B does initial sync (catches up all changes)
    manager_b.initial_sync(None).await.unwrap();

    // B should have everything
    assert!(db_b.get_collective(cid).unwrap().is_some());
    for id in &exp_ids {
        assert!(
            db_b.get_experience(*id).unwrap().is_some(),
            "Experience {} should be synced",
            id
        );
    }
}

// ============================================================================
// SyncManager lifecycle
// ============================================================================

#[tokio::test]
async fn test_sync_manager_status() {
    let pair = setup_sync_pair();
    assert_eq!(pair.manager_a.status(), SyncStatus::Idle);
}

#[tokio::test]
async fn test_sync_manager_start_stop() {
    let mut pair = setup_sync_pair();

    pair.manager_a.start().await.unwrap();
    // Give the background loop a moment
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    pair.manager_a.stop().await.unwrap();
    assert_eq!(pair.manager_a.status(), SyncStatus::Idle);
}

// ============================================================================
// Selective sync (collective filter)
// ============================================================================

#[tokio::test]
async fn test_selective_collective_sync() {
    let (db_a, _dir_a) = open_db();
    let (db_b, _dir_b) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();

    let cid_yes = db_a.create_collective("yes").unwrap();
    let cid_no = db_a.create_collective("no").unwrap();

    let exp_yes = db_a.record_experience(minimal_exp(cid_yes)).unwrap();
    let exp_no = db_a.record_experience(minimal_exp(cid_no)).unwrap();

    // Only sync cid_yes
    let config = SyncConfig {
        collectives: Some(vec![cid_yes]),
        ..sync_config()
    };

    let mut manager_a = SyncManager::new(Arc::clone(&db_a), Box::new(transport_a), config.clone());
    let mut manager_b = SyncManager::new(Arc::clone(&db_b), Box::new(transport_b), config);

    manager_a.sync_once().await.unwrap();
    manager_b.sync_once().await.unwrap();

    // B should have the filtered collective's experience
    assert!(db_b.get_collective(cid_yes).unwrap().is_some());
    assert!(db_b.get_experience(exp_yes).unwrap().is_some());

    // B should NOT have the excluded collective
    assert!(
        db_b.get_collective(cid_no).unwrap().is_none(),
        "Excluded collective should not sync"
    );
    assert!(
        db_b.get_experience(exp_no).unwrap().is_none(),
        "Excluded experience should not sync"
    );
}

// ============================================================================
// WAL compaction vs. unpushed local events (#9 — r1.s1.w1)
// ============================================================================

/// A remote *pull* position must never let `compact_wal` delete local events
/// that have not been *pushed* yet (issue #9). Push and pull positions are
/// tracked separately per peer, and compaction trusts only the push side.
#[tokio::test]
async fn test_compact_wal_keeps_unpushed_local_events() {
    let (db_a, _dir_a) = open_db();
    let (db_b, _dir_b) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();
    let peer_of_a = transport_a.instance_id();

    let mut manager_a = SyncManager::new(Arc::clone(&db_a), Box::new(transport_a), sync_config());
    // B pushes and pulls through separate managers over the same transport so
    // B's own seeding push does not advance B's pull position past A's events.
    let mut manager_b_push = SyncManager::new(
        Arc::clone(&db_b),
        Box::new(transport_b.clone()),
        SyncConfig {
            direction: SyncDirection::PushOnly,
            ..sync_config()
        },
    );
    let mut manager_b_pull = SyncManager::new(
        Arc::clone(&db_b),
        Box::new(transport_b),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    );

    // Seed B so A's pull position lands well above A's own WAL.
    let cid = db_b.create_collective("shared").unwrap();
    for _ in 0..8 {
        db_b.record_experience(minimal_exp(cid)).unwrap();
    }
    manager_b_push.sync_once().await.unwrap();

    // initial_sync A <- B: A's pull position for B is now 9 (collective + 8).
    manager_a.initial_sync(None).await.unwrap();
    assert!(
        db_a.get_collective(cid).unwrap().is_some(),
        "A should have pulled B's collective"
    );

    // A writes locally: WAL seq 1 (sync-applied changes record no WAL events).
    let local_id = db_a.record_experience(minimal_exp(cid)).unwrap();
    assert_eq!(db_a.get_current_sequence().unwrap(), 1);

    // Nothing has been pushed to B yet -> compaction must delete nothing.
    let deleted = db_a.compact_wal().unwrap();
    assert_eq!(
        deleted, 0,
        "compaction deleted unpushed local events off a remote pull position (#9)"
    );
    let pending = db_a.storage_for_test().poll_sync_events(0, 100).unwrap();
    assert_eq!(
        pending.len(),
        1,
        "the unpushed local event must survive compaction"
    );
    assert_eq!(pending[0].0, 1);

    // Push A -> B; B pulls and has the record.
    manager_a.sync_once().await.unwrap();
    manager_b_pull.sync_once().await.unwrap();
    assert!(
        db_b.get_experience(local_id).unwrap().is_some(),
        "B should have A's local record after the push"
    );

    // After the push, compaction may delete at most up to A's push position (1).
    // A second local write above that position must survive.
    db_a.record_experience(minimal_exp(cid)).unwrap();
    assert_eq!(db_a.get_current_sequence().unwrap(), 2);
    let cursor = db_a
        .storage_for_test()
        .load_sync_cursor(&peer_of_a)
        .unwrap()
        .expect("A persisted a cursor for B");
    assert_eq!(cursor.push_sequence, 1, "push side = what B acknowledged");
    assert_eq!(
        cursor.pull_sequence, 9,
        "pull side = B's WAL position, untouched by the push"
    );
    let deleted = db_a.compact_wal().unwrap();
    assert!(
        deleted <= cursor.push_sequence,
        "compaction must not exceed the push position"
    );
    assert_eq!(deleted, 1, "compaction deletes exactly the pushed event");
    let remaining = db_a.storage_for_test().poll_sync_events(0, 100).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].0, 2,
        "the unpushed event above the push position survives"
    );
}

// ============================================================================
// Skew visibility (r1.s1.w3 — #13, veto fold C2)
// ============================================================================

/// A pulled change whose `last_reinforced` lies beyond
/// `now + max_clock_skew_ms` shows up in `SyncManager::stats()` and is merged
/// unchanged — visible, never clamped.
#[tokio::test]
async fn manager_stats_count_skewed_last_reinforced() {
    use std::collections::BTreeMap;

    use pulsedb::sync::transport::SyncTransport;
    use pulsedb::sync::types::{
        SerializableExperienceUpdate, SyncChange, SyncEntityType, SyncPayload, SyncStats,
    };
    use pulsedb::Timestamp;

    let (db_a, _dir_a) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();
    let cid = db_a.create_collective("skew-stats").unwrap();
    let exp_id = db_a.record_experience(minimal_exp(cid)).unwrap();

    let config = SyncConfig {
        direction: SyncDirection::PullOnly,
        ..sync_config()
    };
    let mut manager_a = SyncManager::new(Arc::clone(&db_a), Box::new(transport_a), config.clone());
    assert_eq!(manager_a.stats(), SyncStats::default());

    // The peer pushes a reinforcement whose timestamp is a day past the bound.
    let allowance = i64::try_from(config.max_clock_skew_ms).unwrap();
    let skewed = Timestamp::from_millis(Timestamp::now().as_millis() + allowance + 86_400_000);
    let peer = transport_b.instance_id();
    let change = SyncChange {
        sequence: 1_000,
        source_instance: peer,
        collective_id: cid,
        entity_type: SyncEntityType::Experience,
        payload: SyncPayload::ExperienceUpdated {
            id: exp_id,
            update: SerializableExperienceUpdate {
                applications: Some(BTreeMap::from([(peer, 1)])),
                last_reinforced: Some(skewed),
                ..Default::default()
            },
            timestamp: Timestamp::now(),
        },
        timestamp: Timestamp::now(),
    };
    transport_b.push_changes(vec![change]).await.unwrap();

    manager_a.sync_once().await.unwrap();

    assert_eq!(
        manager_a.stats(),
        SyncStats {
            skewed_timestamps: 1
        },
        "the skewed reinforcement is visible in the manager's stats"
    );
    let stored = db_a.get_experience(exp_id).unwrap().unwrap();
    assert_eq!(stored.last_reinforced, skewed, "counted, not clamped");
    assert_eq!(stored.applications.get(&peer), Some(&1));

    // Stats are cumulative across cycles and untouched by a clean cycle.
    manager_a.sync_once().await.unwrap();
    assert_eq!(manager_a.stats().skewed_timestamps, 1);
}

// ============================================================================
// Empty pulls still register the peer (#9 follow-up — PR #88 review)
// ============================================================================

/// A pull that returns nothing must still persist the peer's cursor record.
/// The record is what represents the peer in the cursor store, and
/// `compact_wal` needs to see its `push_sequence == 0` to stay blocked. Without
/// it a `PullOnly` peer is invisible, and once other peers acknowledge the
/// local WAL, compaction deletes events this peer was never sent.
#[tokio::test]
async fn empty_pull_still_registers_the_peer_and_blocks_compaction() {
    let (db_a, _dir_a) = open_db();
    let (_db_b, _dir_b) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();
    let peer_of_a = transport_a.instance_id();
    drop(transport_b);

    // B has nothing to offer, so A's pull comes back empty.
    let mut manager_a = SyncManager::new(
        Arc::clone(&db_a),
        Box::new(transport_a),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    );
    manager_a.sync_once().await.unwrap();

    // The peer must nevertheless be on record, at push_sequence 0.
    let cursor = db_a
        .storage_for_test()
        .load_sync_cursor(&peer_of_a)
        .unwrap()
        .expect("an empty pull must still register the peer");
    assert_eq!(
        cursor.push_sequence, 0,
        "nothing has been pushed to this peer"
    );

    // A writes locally; compaction must not touch it while that peer sits at 0.
    let cid = db_a.create_collective("local-only").unwrap();
    db_a.record_experience(minimal_exp(cid)).unwrap();
    assert_eq!(
        db_a.compact_wal().unwrap(),
        0,
        "a peer at push_sequence 0 blocks compaction"
    );
}

/// The pull side must not step over a change that failed to apply. `apply_batch`
/// records per-change failures rather than returning an error, so persisting the
/// server's reported position would skip the failed sequence permanently.
#[tokio::test]
async fn pull_position_does_not_advance_past_a_change_that_failed_to_apply() {
    use std::collections::BTreeMap;

    use pulsedb::sync::transport::SyncTransport;
    use pulsedb::sync::types::{
        SerializableExperienceUpdate, SyncChange, SyncEntityType, SyncPayload,
    };

    let (db_a, _dir_a) = open_db();
    let (transport_a, transport_b) = InMemorySyncTransport::new_pair();
    let peer_of_a = transport_a.instance_id();
    let cid = db_a.create_collective("pull-ack-bound").unwrap();
    let exp_id = db_a.record_experience(minimal_exp(cid)).unwrap();

    // A payload the applier refuses: more G-counter buckets than it accepts.
    let mut buckets = BTreeMap::new();
    for i in 0..=65_536u128 {
        buckets.insert(
            pulsedb::sync::types::InstanceId::from_bytes(i.to_le_bytes()),
            1u32,
        );
    }

    let change_at = |seq: u64, applications| SyncChange {
        sequence: seq,
        source_instance: peer_of_a,
        collective_id: cid,
        entity_type: SyncEntityType::Experience,
        payload: SyncPayload::ExperienceUpdated {
            id: exp_id,
            update: SerializableExperienceUpdate {
                applications,
                ..Default::default()
            },
            timestamp: pulsedb::Timestamp::from_millis(0),
        },
        timestamp: pulsedb::Timestamp::now(),
    };

    // seq 7 applies, seq 8 is refused, seq 9 would apply.
    transport_b
        .push_changes(vec![
            change_at(7, None),
            change_at(8, Some(buckets)),
            change_at(9, None),
        ])
        .await
        .unwrap();

    let mut manager_a = SyncManager::new(
        Arc::clone(&db_a),
        Box::new(transport_a),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    );
    manager_a.sync_once().await.unwrap();

    let cursor = db_a
        .storage_for_test()
        .load_sync_cursor(&peer_of_a)
        .unwrap()
        .expect("the peer must be on record");
    assert_eq!(
        cursor.pull_sequence, 7,
        "the pull position must stop at the last change that applied, so the \
         refused one is fetched again rather than skipped forever"
    );
}

// ============================================================================
// initial_sync terminates on a stalled position (PR #88 review, class B)
// and reports completion only when it achieved it (class L)
// ============================================================================

/// A server that filtered every event it polled returns `changes: []`,
/// `has_more: true` and an UNADVANCED cursor. `initial_sync` must read the
/// stalled position as "no progress" and stop, not re-issue the identical
/// request forever — and must report the stop as an error, because the changes
/// beyond that page were never reached.
///
/// The transport fails the request once it has been asked more times than the
/// guard should ever allow, so a regression surfaces as a failed assertion
/// rather than a hung test: the loop's awaits all resolve immediately, so on a
/// current-thread runtime a spin never yields long enough for a timeout to
/// fire.
#[tokio::test]
async fn initial_sync_terminates_on_an_empty_batch_that_claims_more() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use pulsedb::sync::transport::SyncTransport;
    use pulsedb::sync::types::{
        HandshakeRequest, HandshakeResponse, InstanceId, PullRequest, PullResponse, PushResponse,
        SyncChange, SyncPosition,
    };
    use pulsedb::sync::{SyncError, SYNC_PROTOCOL_VERSION};

    /// More pulls than a correct `initial_sync` can possibly issue here.
    const SPIN_TRIPWIRE: usize = 8;

    /// Always answers with an empty batch, `has_more: true`, and the sequence
    /// that was asked for — the shape a fully-filtered poll produces.
    struct StalledPullTransport {
        peer: InstanceId,
        pulls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SyncTransport for StalledPullTransport {
        async fn handshake(
            &self,
            _request: HandshakeRequest,
        ) -> Result<HandshakeResponse, SyncError> {
            Ok(HandshakeResponse {
                instance_id: self.peer,
                protocol_version: SYNC_PROTOCOL_VERSION,
                accepted: true,
                reason: None,
            })
        }

        async fn push_changes(&self, _changes: Vec<SyncChange>) -> Result<PushResponse, SyncError> {
            unreachable!("initial_sync never pushes");
        }

        async fn pull_changes(&self, request: PullRequest) -> Result<PullResponse, SyncError> {
            if self.pulls.fetch_add(1, Ordering::SeqCst) >= SPIN_TRIPWIRE {
                return Err(SyncError::transport("initial_sync is spinning"));
            }
            Ok(PullResponse {
                changes: Vec::new(),
                has_more: true,
                new_cursor: SyncPosition::new(self.peer, request.cursor.sequence),
            })
        }

        async fn health_check(&self) -> Result<(), SyncError> {
            Ok(())
        }
    }

    let (db, _dir) = open_db();
    let pulls = Arc::new(AtomicUsize::new(0));
    let transport = StalledPullTransport {
        peer: InstanceId::new(),
        pulls: Arc::clone(&pulls),
    };

    let mut manager = SyncManager::new(
        Arc::clone(&db),
        Box::new(transport),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    );

    let error = manager
        .initial_sync(None)
        .await
        .expect_err("a peer that promises more and will not advance has not caught us up");

    assert!(
        error.is_catch_up_incomplete(),
        "the stall must surface as the typed catch-up error, got: {error}"
    );
    assert!(
        error.to_string().contains("did not advance the cursor"),
        "the error must name what stopped it, got: {error}"
    );
    assert_eq!(
        pulls.load(Ordering::SeqCst),
        1,
        "one request is enough to learn the position will not move"
    );
}

/// A pull transport that serves ONE scripted page and then reports itself
/// exhausted, with the same bounded tripwire as the stall test above: it fails
/// the request once it has been asked more times than a correct `initial_sync`
/// can ask, so a regression surfaces as an assertion rather than a hang.
struct ScriptedPullTransport {
    peer: pulsedb::sync::types::InstanceId,
    pulls: Arc<std::sync::atomic::AtomicUsize>,
    page: std::sync::Mutex<Vec<pulsedb::sync::types::SyncChange>>,
    has_more: bool,
}

impl ScriptedPullTransport {
    /// More pulls than a correct `initial_sync` can issue against one page.
    const SPIN_TRIPWIRE: usize = 8;

    fn new(
        peer: pulsedb::sync::types::InstanceId,
        pulls: &Arc<std::sync::atomic::AtomicUsize>,
        page: Vec<pulsedb::sync::types::SyncChange>,
        has_more: bool,
    ) -> Self {
        Self {
            peer,
            pulls: Arc::clone(pulls),
            page: std::sync::Mutex::new(page),
            has_more,
        }
    }
}

#[async_trait::async_trait]
impl pulsedb::sync::transport::SyncTransport for ScriptedPullTransport {
    async fn handshake(
        &self,
        _request: pulsedb::sync::types::HandshakeRequest,
    ) -> Result<pulsedb::sync::types::HandshakeResponse, pulsedb::sync::SyncError> {
        Ok(pulsedb::sync::types::HandshakeResponse {
            instance_id: self.peer,
            protocol_version: pulsedb::sync::SYNC_PROTOCOL_VERSION,
            accepted: true,
            reason: None,
        })
    }

    async fn push_changes(
        &self,
        _changes: Vec<pulsedb::sync::types::SyncChange>,
    ) -> Result<pulsedb::sync::types::PushResponse, pulsedb::sync::SyncError> {
        unreachable!("initial_sync never pushes");
    }

    async fn pull_changes(
        &self,
        request: pulsedb::sync::types::PullRequest,
    ) -> Result<pulsedb::sync::types::PullResponse, pulsedb::sync::SyncError> {
        use std::sync::atomic::Ordering;

        if self.pulls.fetch_add(1, Ordering::SeqCst) >= Self::SPIN_TRIPWIRE {
            return Err(pulsedb::sync::SyncError::transport(
                "initial_sync is spinning",
            ));
        }
        let page = std::mem::take(&mut *self.page.lock().unwrap());
        // Once the page is served the peer is exhausted, whatever it said the
        // first time.
        let has_more = !page.is_empty() && self.has_more;
        let new_seq = page
            .last()
            .map_or(request.cursor.sequence, |change| change.sequence);
        Ok(pulsedb::sync::types::PullResponse {
            changes: page,
            has_more,
            new_cursor: pulsedb::sync::types::SyncPosition::new(self.peer, new_seq),
        })
    }

    async fn health_check(&self) -> Result<(), pulsedb::sync::SyncError> {
        Ok(())
    }
}

/// Builds a manager pulling from a one-page scripted peer.
fn catchup_manager(
    db: &Arc<PulseDB>,
    peer: pulsedb::sync::types::InstanceId,
    pulls: &Arc<std::sync::atomic::AtomicUsize>,
    page: Vec<pulsedb::sync::types::SyncChange>,
    has_more: bool,
) -> SyncManager {
    SyncManager::new(
        Arc::clone(db),
        Box::new(ScriptedPullTransport::new(peer, pulls, page, has_more)),
        SyncConfig {
            direction: SyncDirection::PullOnly,
            ..sync_config()
        },
    )
}

/// A page the peer reports as its last, with every change applying, IS a
/// completed catch-up.
#[tokio::test]
async fn initial_sync_completes_on_an_exhausted_page() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (db, _dir) = open_db();
    let pulls = Arc::new(AtomicUsize::new(0));
    let collective = CollectiveId::new();
    let mut manager = catchup_manager(
        &db,
        pulsedb::sync::types::InstanceId::new(),
        &pulls,
        vec![collective_change(1, collective, "catchup-complete")],
        false,
    );

    manager
        .initial_sync(None)
        .await
        .expect("an exhausted page with everything applied is a completed catch-up");

    assert!(db.get_collective(collective).unwrap().is_some());
    assert_eq!(pulls.load(Ordering::SeqCst), 1);
}

/// An idempotent SKIP is a successful outcome — it is the ordinary shape of a
/// re-sync — so a page of nothing but skips still completes. This is the
/// distinction `ApplyResult::failed` exists to draw: `skipped` counts these too.
#[tokio::test]
async fn initial_sync_completes_on_a_page_of_idempotent_skips() {
    use std::sync::atomic::AtomicUsize;

    let (db, _dir) = open_db();
    let pulls = Arc::new(AtomicUsize::new(0));
    // Deleting an experience this store never had is the idempotent skip.
    let absent = ExperienceId::new();
    let peer = pulsedb::sync::types::InstanceId::new();
    let mut manager = catchup_manager(&db, peer, &pulls, vec![delete_change(1, absent)], false);

    manager
        .initial_sync(None)
        .await
        .expect("a page of idempotent skips is a completed catch-up, not a failure");

    let cursor = db
        .storage_for_test()
        .load_sync_cursor(&peer)
        .unwrap()
        .expect("the peer must be on record");
    assert_eq!(
        cursor.pull_sequence, 1,
        "the skipped change was handled, so the position moves past it"
    );
}

/// An exhausted page is not enough: a change that FAILED to apply means the
/// store is not caught up, so `initial_sync` must not report success.
#[tokio::test]
async fn initial_sync_refuses_completion_when_a_change_failed_to_apply() {
    use std::sync::atomic::AtomicUsize;

    let (db, _dir) = open_db();
    let pulls = Arc::new(AtomicUsize::new(0));
    let collective = CollectiveId::new();
    let page = vec![
        collective_change(1, collective, "catchup-partial"),
        poison_change(2, collective),
    ];
    let peer = pulsedb::sync::types::InstanceId::new();
    let mut manager = catchup_manager(&db, peer, &pulls, page, false);

    let error = manager
        .initial_sync(None)
        .await
        .expect_err("a change that never applied is not a completed catch-up");

    assert!(
        error.is_catch_up_incomplete(),
        "an apply failure must surface as the typed catch-up error, got: {error}"
    );
    assert!(
        error.to_string().contains("failed to apply"),
        "the error must name what stopped it, got: {error}"
    );
    let cursor = db
        .storage_for_test()
        .load_sync_cursor(&peer)
        .unwrap()
        .expect("the peer must be on record");
    assert_eq!(
        cursor.pull_sequence, 1,
        "the position stops at the last change that applied, so the failed one \
         is fetched again"
    );
}

/// A `CollectiveCreated` change at `sequence`.
fn collective_change(
    sequence: u64,
    id: CollectiveId,
    name: &str,
) -> pulsedb::sync::types::SyncChange {
    pulsedb::sync::types::SyncChange {
        sequence,
        source_instance: pulsedb::sync::types::InstanceId::new(),
        collective_id: id,
        entity_type: pulsedb::sync::types::SyncEntityType::Collective,
        payload: pulsedb::sync::types::SyncPayload::CollectiveCreated(pulsedb::Collective {
            id,
            name: name.to_string(),
            owner_id: None,
            embedding_dimension: 384,
            created_at: pulsedb::Timestamp::now(),
            updated_at: pulsedb::Timestamp::now(),
        }),
        timestamp: pulsedb::Timestamp::now(),
    }
}

/// An `ExperienceDeleted` change at `sequence` — an idempotent skip when the
/// experience is absent locally.
fn delete_change(sequence: u64, id: ExperienceId) -> pulsedb::sync::types::SyncChange {
    pulsedb::sync::types::SyncChange {
        sequence,
        source_instance: pulsedb::sync::types::InstanceId::new(),
        collective_id: CollectiveId::new(),
        entity_type: pulsedb::sync::types::SyncEntityType::Experience,
        payload: pulsedb::sync::types::SyncPayload::ExperienceDeleted {
            id,
            timestamp: pulsedb::Timestamp::from_millis(0),
        },
        timestamp: pulsedb::Timestamp::now(),
    }
}

/// A change the applier REFUSES: more `applications` G-counter buckets than it
/// accepts from a peer.
fn poison_change(sequence: u64, collective: CollectiveId) -> pulsedb::sync::types::SyncChange {
    use std::collections::BTreeMap;

    let mut buckets = BTreeMap::new();
    for i in 0..=65_536u128 {
        buckets.insert(
            pulsedb::sync::types::InstanceId::from_bytes(i.to_le_bytes()),
            1u32,
        );
    }
    pulsedb::sync::types::SyncChange {
        sequence,
        source_instance: pulsedb::sync::types::InstanceId::new(),
        collective_id: collective,
        entity_type: pulsedb::sync::types::SyncEntityType::Experience,
        payload: pulsedb::sync::types::SyncPayload::ExperienceUpdated {
            id: ExperienceId::new(),
            update: pulsedb::sync::types::SerializableExperienceUpdate {
                applications: Some(buckets),
                ..Default::default()
            },
            timestamp: pulsedb::Timestamp::from_millis(0),
        },
        timestamp: pulsedb::Timestamp::now(),
    }
}
