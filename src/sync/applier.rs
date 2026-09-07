//! Remote change applier — applies changes received from a remote peer.
//!
//! The `RemoteChangeApplier` receives batches of `SyncChange` from pull
//! responses and applies them to the local database. It handles:
//! - Echo prevention via [`SyncApplyGuard`]
//! - Idempotent creates (skip if entity exists)
//! - Idempotent deletes (skip if entity missing)
//! - Conflict resolution for experience updates

use std::sync::{Arc, Mutex};

use tracing::{debug, instrument, trace, warn};

use crate::db::PulseDB;
use crate::experience::ExperienceUpdate;
use crate::types::Timestamp;

use super::config::{ConflictResolution, SyncConfig};
use super::error::SyncError;
use super::guard::SyncApplyGuard;
use super::types::{SyncChange, SyncPayload, SyncStats};

/// Upper bound on the number of per-instance buckets accepted in a single
/// experience's `applications` G-counter from a remote peer. Each bucket is one
/// distinct replica that reinforced the experience, so realistic counts are in
/// the tens-to-thousands even for large fleets; a payload exceeding this is
/// treated as malformed/hostile and rejected to prevent unbounded memory growth
/// and persistent state bloat (resource-exhaustion DoS) during sync apply.
const MAX_SYNC_APPLICATION_BUCKETS: usize = 65_536;

/// Result of applying a batch of remote changes.
#[derive(Clone, Debug, Default)]
pub struct ApplyResult {
    /// Number of changes successfully applied.
    pub applied: usize,
    /// Number of changes skipped (idempotent / filtered).
    pub skipped: usize,
    /// Number of changes where conflict resolution was used.
    pub conflicts: usize,
    /// Highest sequence that is safe to acknowledge: every change at or below
    /// it was applied, resolved, or idempotently skipped. `None` when the very
    /// first change failed.
    ///
    /// A change that *errored* stops this advancing, and nothing after it
    /// counts either — the peer must be able to retry it, and the sender's
    /// `push_sequence` (which is what `compact_wal` trusts) is derived from
    /// this value.
    pub safe_through: Option<u64>,
    /// Number of changes in this batch whose incoming `last_reinforced` lay
    /// beyond `now + SyncConfig::max_clock_skew_ms` (#13).
    ///
    /// Detection only: each is logged at `warn` and then merged unchanged
    /// (FR-031 max-merge, r1 veto fold C2). Never serialized on the wire.
    pub skewed_timestamps: u64,
}

impl ApplyResult {
    /// Folds this batch's counters into a cumulative, local-only [`SyncStats`].
    pub(crate) fn record_into(&self, stats: &Mutex<SyncStats>) {
        let mut stats = stats.lock().unwrap_or_else(|e| e.into_inner());
        stats.skewed_timestamps = stats
            .skewed_timestamps
            .saturating_add(self.skewed_timestamps);
    }
}

/// Applies remote sync changes to the local PulseDB instance.
pub(crate) struct RemoteChangeApplier {
    db: Arc<PulseDB>,
    config: SyncConfig,
}

impl RemoteChangeApplier {
    /// Creates a new applier.
    pub fn new(db: Arc<PulseDB>, config: SyncConfig) -> Self {
        Self { db, config }
    }

    /// Applies a batch of remote changes to the local database.
    ///
    /// Each change is applied under a [`SyncApplyGuard`] to prevent
    /// WAL re-emission (echo prevention). Changes are applied in order.
    #[instrument(skip(self, changes), fields(batch_size = changes.len()))]
    pub fn apply_batch(&self, changes: Vec<SyncChange>) -> Result<ApplyResult, SyncError> {
        let mut result = ApplyResult::default();

        // Once a change errors, nothing at or after it may be acknowledged:
        // the sender derives its push position from `safe_through`, and
        // compaction deletes below that position.
        let mut halted = false;

        for change in changes {
            let sequence = change.sequence;
            // #13: surface a skewed reinforcement timestamp BEFORE the apply,
            // so it is counted whatever the apply's outcome. Detection never
            // alters the change — it is applied exactly as received.
            if self.detect_skew(&change) {
                result.skewed_timestamps += 1;
            }
            match self.apply_single(change) {
                Ok(ApplyOutcome::Applied) => {
                    result.applied += 1;
                    if !halted {
                        result.safe_through = Some(sequence);
                    }
                }
                Ok(ApplyOutcome::Skipped) => {
                    result.skipped += 1;
                    if !halted {
                        result.safe_through = Some(sequence);
                    }
                }
                Ok(ApplyOutcome::ConflictResolved) => {
                    result.applied += 1;
                    result.conflicts += 1;
                    if !halted {
                        result.safe_through = Some(sequence);
                    }
                }
                Err(e) => {
                    warn!(sequence, "Failed to apply sync change: {}", e);
                    // Continue applying the rest — a later change may be
                    // independent — but never acknowledge past this one.
                    halted = true;
                    result.skipped += 1;
                }
            }
        }

        debug!(
            applied = result.applied,
            skipped = result.skipped,
            conflicts = result.conflicts,
            skewed_timestamps = result.skewed_timestamps,
            safe_through = result.safe_through,
            "Applied remote change batch"
        );
        Ok(result)
    }

    /// #13 (r1 veto fold C2) — skew **detection**, never correction.
    ///
    /// Returns `true` when the change carries a reinforcement timestamp beyond
    /// `now + max_clock_skew_ms` — at either apply site: the non-collision
    /// create (`ExperienceCreated`, timestamp written as-is) and the counter
    /// merge (`ExperienceCreated` collision or `ExperienceUpdated`, FR-031
    /// max-merge). It logs the condition at `warn` with the peer, the
    /// experience id and the skew, and leaves the change untouched: the merged
    /// value stays byte-for-byte what max-merge produces, so convergence is
    /// unaffected and the skew is visible in logs and [`SyncStats`] instead of
    /// silently freezing decay. A bound that also corrects needs a
    /// record-carried reference — protocol v5 (Release 2).
    fn detect_skew(&self, change: &SyncChange) -> bool {
        let (id, incoming) = match &change.payload {
            SyncPayload::ExperienceCreated(experience) => {
                (experience.id, experience.last_reinforced)
            }
            SyncPayload::ExperienceUpdated { id, update, .. } => match update.last_reinforced {
                Some(incoming) => (*id, incoming),
                None => return false,
            },
            _ => return false,
        };

        let now_ms = Timestamp::now().as_millis();
        let allowance = i64::try_from(self.config.max_clock_skew_ms).unwrap_or(i64::MAX);
        let bound = now_ms.saturating_add(allowance);
        if incoming.as_millis() <= bound {
            return false;
        }

        warn!(
            peer = %change.source_instance,
            experience_id = %id,
            incoming_last_reinforced_ms = incoming.as_millis(),
            skew_ms = incoming.as_millis().saturating_sub(now_ms),
            max_clock_skew_ms = self.config.max_clock_skew_ms,
            "Incoming last_reinforced is beyond the clock-skew bound; merged unchanged (bound is advisory until protocol v5)"
        );
        true
    }

    /// Applies a single remote change, returning the outcome.
    fn apply_single(&self, change: SyncChange) -> Result<ApplyOutcome, SyncError> {
        let _guard = SyncApplyGuard::enter();

        let map_err = |e: crate::error::PulseDBError| {
            SyncError::transport(format!("Failed to apply sync change: {}", e))
        };

        match change.payload {
            // ─── Experience ──────────────────────────────────────────
            SyncPayload::ExperienceCreated(experience) => {
                let id = experience.id;
                if experience.applications.len() > MAX_SYNC_APPLICATION_BUCKETS {
                    return Err(SyncError::invalid_payload(format!(
                        "experience {id} sync create carries {} application buckets (max {MAX_SYNC_APPLICATION_BUCKETS})",
                        experience.applications.len()
                    )));
                }
                if self.db.get_experience(id).map_err(map_err)?.is_some() {
                    let merged = self
                        .db
                        .apply_synced_experience_counter_merge(
                            id,
                            &experience.applications,
                            Some(experience.last_reinforced),
                        )
                        .map_err(map_err)?;
                    if merged {
                        trace!(id = %id, "Merged ExperienceCreated counter collision");
                        return Ok(ApplyOutcome::Applied);
                    }
                    trace!(id = %id, "Skipping ExperienceCreated: already exists");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db
                    .apply_synced_experience(experience)
                    .map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            SyncPayload::ExperienceUpdated {
                id,
                update,
                timestamp,
                ..
            } => {
                if let Some(incoming) = update.applications.as_ref() {
                    if incoming.len() > MAX_SYNC_APPLICATION_BUCKETS {
                        return Err(SyncError::invalid_payload(format!(
                            "experience {id} sync update carries {} application buckets (max {MAX_SYNC_APPLICATION_BUCKETS})",
                            incoming.len()
                        )));
                    }
                }
                let applications = update.applications.as_ref().cloned().unwrap_or_default();
                let last_reinforced = update.last_reinforced;
                let has_counter_merge =
                    update.applications.is_some() || update.last_reinforced.is_some();
                let counter_merged = if has_counter_merge {
                    self.db
                        .apply_synced_experience_counter_merge(id, &applications, last_reinforced)
                        .map_err(map_err)?
                } else {
                    false
                };

                let mut apply_scalar_update = true;
                if self.config.conflict_resolution == ConflictResolution::LastWriteWins {
                    if let Some(local) = self.db.get_experience(id).map_err(map_err)? {
                        if local.timestamp > timestamp {
                            trace!(id = %id, "Skipping scalar ExperienceUpdated fields: local is newer (LastWriteWins)");
                            apply_scalar_update = false;
                        }
                    }
                }

                if !apply_scalar_update {
                    return if counter_merged {
                        Ok(ApplyOutcome::ConflictResolved)
                    } else {
                        Ok(ApplyOutcome::Skipped)
                    };
                }

                // ServerWins: always apply. LastWriteWins: remote is newer or equal.
                let experience_update: ExperienceUpdate = update.into();
                self.db
                    .apply_synced_experience_update(id, experience_update)
                    .map_err(map_err)?;
                if self.config.conflict_resolution == ConflictResolution::LastWriteWins {
                    Ok(ApplyOutcome::ConflictResolved)
                } else {
                    Ok(ApplyOutcome::Applied)
                }
            }

            SyncPayload::ExperienceArchived { id, .. } => {
                let update = ExperienceUpdate {
                    archived: Some(true),
                    ..Default::default()
                };
                // Skip if experience doesn't exist
                if self.db.get_experience(id).map_err(map_err)?.is_none() {
                    trace!(id = %id, "Skipping ExperienceArchived: not found");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db
                    .apply_synced_experience_update(id, update)
                    .map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            SyncPayload::ExperienceDeleted { id, .. } => {
                // Idempotent: skip if already gone
                if self.db.get_experience(id).map_err(map_err)?.is_none() {
                    trace!(id = %id, "Skipping ExperienceDeleted: not found");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db
                    .apply_synced_experience_delete(id)
                    .map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            // ─── Relation ────────────────────────────────────────────
            SyncPayload::RelationCreated(relation) => {
                let id = relation.id;
                // Idempotent: skip if already exists
                if self.db.get_relation(id).map_err(map_err)?.is_some() {
                    trace!(id = %id, "Skipping RelationCreated: already exists");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db.apply_synced_relation(relation).map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            SyncPayload::RelationDeleted { id, .. } => {
                // Idempotent: skip if already gone
                if self.db.get_relation(id).map_err(map_err)?.is_none() {
                    trace!(id = %id, "Skipping RelationDeleted: not found");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db.apply_synced_relation_delete(id).map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            // ─── Insight ─────────────────────────────────────────────
            SyncPayload::InsightCreated(insight) => {
                let id = insight.id;
                // Idempotent: skip if already exists
                if self.db.get_insight(id).map_err(map_err)?.is_some() {
                    trace!(id = %id, "Skipping InsightCreated: already exists");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db.apply_synced_insight(insight).map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            SyncPayload::InsightDeleted { id, .. } => {
                // Idempotent: skip if already gone
                if self.db.get_insight(id).map_err(map_err)?.is_none() {
                    trace!(id = %id, "Skipping InsightDeleted: not found");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db.apply_synced_insight_delete(id).map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }

            // ─── Collective ──────────────────────────────────────────
            SyncPayload::CollectiveCreated(collective) => {
                let id = collective.id;
                // Idempotent: skip if already exists
                if self.db.get_collective(id).map_err(map_err)?.is_some() {
                    trace!(id = %id, "Skipping CollectiveCreated: already exists");
                    return Ok(ApplyOutcome::Skipped);
                }
                self.db
                    .apply_synced_collective(collective)
                    .map_err(map_err)?;
                Ok(ApplyOutcome::Applied)
            }
        }
    }
}

/// Internal outcome of applying a single change.
#[derive(Debug)]
enum ApplyOutcome {
    Applied,
    Skipped,
    ConflictResolved,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::*;
    use crate::sync::types::{SerializableExperienceUpdate, SyncEntityType};
    use crate::{
        CollectiveId, Config, ExperienceId, ExperienceType, InstanceId, NewExperience, PulseDB,
        Timestamp,
    };

    fn open_db() -> (Arc<PulseDB>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db = Arc::new(PulseDB::open(dir.path().join("test.db"), Config::default()).unwrap());
        (db, dir)
    }

    fn minimal_exp(cid: CollectiveId) -> NewExperience {
        NewExperience {
            collective_id: cid,
            content: "applier merge test".to_string(),
            experience_type: ExperienceType::Generic { category: None },
            embedding: Some(vec![0.1f32; 384]),
            importance: 0.9,
            ..Default::default()
        }
    }

    fn change(payload: SyncPayload, cid: CollectiveId) -> SyncChange {
        SyncChange {
            sequence: 1,
            source_instance: InstanceId::new(),
            collective_id: cid,
            entity_type: SyncEntityType::Experience,
            payload,
            timestamp: Timestamp::now(),
        }
    }

    #[test]
    fn experience_created_collision_merges_gcounter_fields() {
        let (db, _dir) = open_db();
        let cid = db.create_collective("applier-create-collision").unwrap();
        let exp_id = db.record_experience(minimal_exp(cid)).unwrap();
        let remote_key = InstanceId::new();
        let incoming_last_reinforced = Timestamp::from_millis(i64::MAX);
        let mut remote = db.get_experience(exp_id).unwrap().unwrap();
        remote.applications = BTreeMap::from([(remote_key, 4)]);
        remote.last_reinforced = incoming_last_reinforced;

        let applier = RemoteChangeApplier::new(Arc::clone(&db), SyncConfig::default());
        let outcome = applier
            .apply_single(change(SyncPayload::ExperienceCreated(remote), cid))
            .unwrap();

        assert!(matches!(outcome, ApplyOutcome::Applied));
        let merged = db.get_experience(exp_id).unwrap().unwrap();
        assert_eq!(merged.applications.get(&remote_key), Some(&4));
        assert_eq!(merged.last_reinforced, incoming_last_reinforced);
    }

    #[test]
    fn lww_skip_does_not_skip_gcounter_merge() {
        let (db, _dir) = open_db();
        let cid = db.create_collective("applier-lww-counter").unwrap();
        let exp_id = db.record_experience(minimal_exp(cid)).unwrap();
        let remote_key = InstanceId::new();
        let incoming_last_reinforced = Timestamp::from_millis(i64::MAX);
        let update = SerializableExperienceUpdate {
            importance: Some(0.1),
            applications: Some(BTreeMap::from([(remote_key, 6)])),
            last_reinforced: Some(incoming_last_reinforced),
            ..Default::default()
        };

        let applier = RemoteChangeApplier::new(
            Arc::clone(&db),
            SyncConfig {
                conflict_resolution: ConflictResolution::LastWriteWins,
                ..SyncConfig::default()
            },
        );
        let outcome = applier
            .apply_single(change(
                SyncPayload::ExperienceUpdated {
                    id: exp_id,
                    update,
                    timestamp: Timestamp::from_millis(0),
                },
                cid,
            ))
            .unwrap();

        assert!(matches!(outcome, ApplyOutcome::ConflictResolved));
        let merged = db.get_experience(exp_id).unwrap().unwrap();
        assert_eq!(merged.applications.get(&remote_key), Some(&6));
        assert_eq!(merged.applications(), 6);
        assert_eq!(merged.last_reinforced, incoming_last_reinforced);
        assert!((merged.importance - 0.9).abs() < f32::EPSILON);
    }

    /// A change that fails to apply must stop the acknowledged position dead:
    /// the sender turns `safe_through` into its `push_sequence`, and
    /// `compact_wal` deletes below that, so acknowledging past a rejected
    /// change would let the sender discard a WAL event this peer never stored.
    #[test]
    fn a_rejected_change_stops_the_acknowledged_position() {
        let (db, _dir) = open_db();
        let cid = db.create_collective("applier-ack-bound").unwrap();
        let good_a = db.record_experience(minimal_exp(cid)).unwrap();
        let good_b = db.record_experience(minimal_exp(cid)).unwrap();

        // A hostile payload the applier refuses (same bound as above).
        let mut buckets = BTreeMap::new();
        for i in 0..=(MAX_SYNC_APPLICATION_BUCKETS as u128) {
            buckets.insert(InstanceId::from_bytes(i.to_le_bytes()), 1u32);
        }
        let poison = SerializableExperienceUpdate {
            applications: Some(buckets),
            ..Default::default()
        };
        let benign = SerializableExperienceUpdate {
            importance: Some(0.5),
            ..Default::default()
        };

        let at = |seq: u64, id, update| {
            let mut c = change(
                SyncPayload::ExperienceUpdated {
                    id,
                    update,
                    timestamp: Timestamp::from_millis(0),
                },
                cid,
            );
            c.sequence = seq;
            c
        };
        let batch = vec![
            at(7, good_a, benign.clone()),
            at(8, good_b, poison),
            at(9, good_a, benign),
        ];

        let applier = RemoteChangeApplier::new(Arc::clone(&db), SyncConfig::default());
        let result = applier.apply_batch(batch).unwrap();

        assert_eq!(
            result.safe_through,
            Some(7),
            "acknowledgement must stop at the last change before the failure, \
             not run on to the last change that happened to succeed"
        );
        assert_eq!(result.skipped, 1, "the rejected change counts as skipped");
    }

    #[test]
    fn oversized_application_bucket_map_is_rejected() {
        let (db, _dir) = open_db();
        let cid = db.create_collective("applier-bucket-bound").unwrap();
        let exp_id = db.record_experience(minimal_exp(cid)).unwrap();

        // A hostile peer payload with more buckets than the accepted bound.
        let mut buckets = BTreeMap::new();
        for i in 0..=(MAX_SYNC_APPLICATION_BUCKETS as u128) {
            buckets.insert(InstanceId::from_bytes(i.to_le_bytes()), 1u32);
        }
        assert_eq!(buckets.len(), MAX_SYNC_APPLICATION_BUCKETS + 1);

        let update = SerializableExperienceUpdate {
            applications: Some(buckets),
            ..Default::default()
        };

        let applier = RemoteChangeApplier::new(Arc::clone(&db), SyncConfig::default());
        let result = applier.apply_single(change(
            SyncPayload::ExperienceUpdated {
                id: exp_id,
                update,
                timestamp: Timestamp::now(),
            },
            cid,
        ));

        assert!(
            matches!(result, Err(SyncError::InvalidPayload(_))),
            "oversized application bucket map must be rejected, got {result:?}"
        );
        // Nothing from the oversized map may have been persisted.
        let stored = db.get_experience(exp_id).unwrap().unwrap();
        assert!(stored.applications.len() <= 1);
    }

    /// #13 (veto fold C2): an incoming `last_reinforced` beyond
    /// `now + max_clock_skew_ms` is COUNTED in `ApplyResult::skewed_timestamps`
    /// (and logged) at every apply site, and NEVER clamped, rejected or
    /// re-timestamped — the stored value is byte-for-byte what FR-031's
    /// max-merge produces.
    #[test]
    fn skewed_last_reinforced_is_counted_not_clamped() {
        let (db, _dir) = open_db();
        let cid = db.create_collective("applier-skew").unwrap();
        let exp_id = db.record_experience(minimal_exp(cid)).unwrap();
        let config = SyncConfig::default();
        assert_eq!(config.max_clock_skew_ms, 300_000, "grill Q4 default");
        let applier = RemoteChangeApplier::new(Arc::clone(&db), config.clone());
        let remote_key = InstanceId::new();

        let now_ms = Timestamp::now().as_millis();
        let allowance = i64::try_from(config.max_clock_skew_ms).unwrap();
        // One day past the skew bound.
        let skewed = Timestamp::from_millis(now_ms + allowance + 86_400_000);

        // 1. Counter-merge site (ExperienceUpdated): counted once, merged as-is.
        let update = SerializableExperienceUpdate {
            applications: Some(BTreeMap::from([(remote_key, 2)])),
            last_reinforced: Some(skewed),
            ..Default::default()
        };
        let result = applier
            .apply_batch(vec![change(
                SyncPayload::ExperienceUpdated {
                    id: exp_id,
                    update,
                    timestamp: Timestamp::now(),
                },
                cid,
            )])
            .unwrap();
        assert_eq!(result.skewed_timestamps, 1, "skewed update is counted once");
        assert_eq!(result.applied, 1);
        let stored = db.get_experience(exp_id).unwrap().unwrap();
        assert_eq!(
            stored.last_reinforced, skewed,
            "max-merge result is stored byte-for-byte — not clamped"
        );
        assert_eq!(stored.applications.get(&remote_key), Some(&2));

        // 2. An in-bound timestamp is not counted; the max-merge still holds.
        let in_bound = Timestamp::from_millis(now_ms + 1_000);
        let update = SerializableExperienceUpdate {
            last_reinforced: Some(in_bound),
            ..Default::default()
        };
        let result = applier
            .apply_batch(vec![change(
                SyncPayload::ExperienceUpdated {
                    id: exp_id,
                    update,
                    timestamp: Timestamp::now(),
                },
                cid,
            )])
            .unwrap();
        assert_eq!(
            result.skewed_timestamps, 0,
            "in-bound update is not counted"
        );
        assert_eq!(
            db.get_experience(exp_id).unwrap().unwrap().last_reinforced,
            skewed,
            "max-merge keeps the larger (skewed) value"
        );

        // 3. Non-collision create site (ExperienceCreated, unknown id): counted,
        //    the record is written with its timestamp untouched.
        let mut fresh = db.get_experience(exp_id).unwrap().unwrap();
        fresh.id = ExperienceId::new();
        fresh.last_reinforced = skewed;
        let result = applier
            .apply_batch(vec![change(
                SyncPayload::ExperienceCreated(fresh.clone()),
                cid,
            )])
            .unwrap();
        assert_eq!(result.skewed_timestamps, 1, "fresh create is counted");
        assert_eq!(result.applied, 1);
        assert_eq!(
            db.get_experience(fresh.id)
                .unwrap()
                .unwrap()
                .last_reinforced,
            skewed,
            "fresh create stores the skewed value unchanged"
        );

        // 4. Collision create site (ExperienceCreated, known id): counted and
        //    max-merged unchanged.
        let mut collision = db.get_experience(exp_id).unwrap().unwrap();
        collision.last_reinforced = Timestamp::from_millis(skewed.as_millis() + 1);
        let result = applier
            .apply_batch(vec![change(
                SyncPayload::ExperienceCreated(collision.clone()),
                cid,
            )])
            .unwrap();
        assert_eq!(result.skewed_timestamps, 1, "collision create is counted");
        assert_eq!(
            db.get_experience(exp_id).unwrap().unwrap().last_reinforced,
            collision.last_reinforced,
            "collision max-merge stores the skewed value unchanged"
        );

        // 5. A change with no reinforcement timestamp is never counted.
        let result = applier
            .apply_batch(vec![change(
                SyncPayload::ExperienceArchived {
                    id: exp_id,
                    timestamp: skewed,
                },
                cid,
            )])
            .unwrap();
        assert_eq!(result.skewed_timestamps, 0);
    }
}
