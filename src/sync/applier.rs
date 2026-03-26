//! Remote change applier — applies changes received from a remote peer.
//!
//! The [`RemoteChangeApplier`] receives batches of [`SyncChange`] from pull
//! responses and applies them to the local database. It handles:
//! - Echo prevention via [`SyncApplyGuard`]
//! - Idempotent creates (skip if entity exists)
//! - Idempotent deletes (skip if entity missing)
//! - Conflict resolution for experience updates

use std::sync::Arc;

use tracing::{debug, instrument, trace, warn};

use crate::db::PulseDB;
use crate::experience::ExperienceUpdate;

use super::config::{ConflictResolution, SyncConfig};
use super::error::SyncError;
use super::guard::SyncApplyGuard;
use super::types::{SyncChange, SyncPayload};

/// Result of applying a batch of remote changes.
#[derive(Clone, Debug, Default)]
pub struct ApplyResult {
    /// Number of changes successfully applied.
    pub applied: usize,
    /// Number of changes skipped (idempotent / filtered).
    pub skipped: usize,
    /// Number of changes where conflict resolution was used.
    pub conflicts: usize,
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

        for change in changes {
            match self.apply_single(change) {
                Ok(ApplyOutcome::Applied) => result.applied += 1,
                Ok(ApplyOutcome::Skipped) => result.skipped += 1,
                Ok(ApplyOutcome::ConflictResolved) => {
                    result.applied += 1;
                    result.conflicts += 1;
                }
                Err(e) => {
                    warn!("Failed to apply sync change: {}", e);
                    // Continue applying remaining changes — don't fail the batch
                    result.skipped += 1;
                }
            }
        }

        debug!(
            applied = result.applied,
            skipped = result.skipped,
            conflicts = result.conflicts,
            "Applied remote change batch"
        );
        Ok(result)
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
                // Idempotent: skip if already exists
                if self.db.get_experience(id).map_err(map_err)?.is_some() {
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
                // Conflict resolution
                if self.config.conflict_resolution == ConflictResolution::LastWriteWins {
                    if let Some(local) = self.db.get_experience(id).map_err(map_err)? {
                        if local.timestamp > timestamp {
                            trace!(id = %id, "Skipping ExperienceUpdated: local is newer (LastWriteWins)");
                            return Ok(ApplyOutcome::Skipped);
                        }
                    }
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
enum ApplyOutcome {
    Applied,
    Skipped,
    ConflictResolved,
}
