//! Local change pusher — reads WAL events and pushes them to a remote peer.
//!
//! The [`LocalChangePusher`] polls the WAL for new events, loads full entity
//! data from storage, constructs [`SyncChange`] payloads, and pushes them
//! via the [`SyncTransport`].

use std::sync::Arc;

use tracing::{debug, instrument, trace, warn};

use crate::db::PulseDB;
use crate::storage::schema::{EntityTypeTag, WatchEventRecord, WatchEventTypeTag};
use crate::types::{CollectiveId, ExperienceId, InsightId, RelationId, Timestamp};
use crate::watch::ChangePoller;

use super::config::SyncConfig;
use super::error::SyncError;
use super::transport::SyncTransport;
use super::types::{
    InstanceId, SerializableExperienceUpdate, SyncChange, SyncEntityType, SyncPayload,
};

/// Polls local WAL events and pushes them to a remote peer via transport.
pub(crate) struct LocalChangePusher {
    db: Arc<PulseDB>,
    transport: Arc<dyn SyncTransport>,
    config: SyncConfig,
    poller: ChangePoller,
    local_instance_id: InstanceId,
    peer_instance_id: InstanceId,
}

impl LocalChangePusher {
    /// Creates a new pusher.
    ///
    /// `start_sequence` is the WAL sequence to resume from (0 for fresh sync).
    ///
    /// The poller is built with [`SyncConfig::batch_size`], which is what makes
    /// that setting mean what its rustdoc says — "maximum number of changes per
    /// sync batch". Left on [`ChangePoller`]'s own default (1000, for
    /// non-sync callers) the pusher would collect up to 1000 events whatever
    /// `batch_size` said, and `SyncConfig::validate`'s
    /// `max_request_bytes >= batch_size * MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`
    /// floor would be computed against a number this path never used — passing
    /// the check while the body it builds is refused by the peer's cap on every
    /// cycle, forever.
    pub fn new(
        db: Arc<PulseDB>,
        transport: Arc<dyn SyncTransport>,
        config: SyncConfig,
        local_instance_id: InstanceId,
        peer_instance_id: InstanceId,
        start_sequence: u64,
    ) -> Self {
        let poller =
            ChangePoller::from_sequence(start_sequence).with_batch_limit(config.batch_size);
        Self {
            db,
            transport,
            config,
            poller,
            local_instance_id,
            peer_instance_id,
        }
    }

    /// Pushes all pending local changes to the remote peer.
    ///
    /// Returns the number of changes successfully pushed.
    #[instrument(skip(self), fields(peer = %self.peer_instance_id))]
    pub async fn push_pending(&mut self) -> Result<usize, SyncError> {
        let storage = self.db.storage_for_test(); // pub accessor
        let events = self
            .poller
            .poll_sync_events(storage)
            .map_err(|e| SyncError::transport(format!("Failed to poll WAL events: {}", e)))?;

        if events.is_empty() {
            return Ok(0);
        }

        let mut changes = Vec::with_capacity(events.len());
        for (sequence, record) in &events {
            if let Some(change) = self.record_to_change(*sequence, record)? {
                changes.push(change);
            }
        }

        if changes.is_empty() {
            // All events were filtered or entities were deleted — nothing to
            // acknowledge. Advance the push position past them: they will never
            // be pushed to this peer, so compaction may reclaim them.
            self.save_push_cursor(self.poller.last_sequence())?;
            return Ok(0);
        }

        let count = changes.len();
        let response = self.transport.push_changes(changes).await?;

        // Persist the position the peer ACKNOWLEDGED, bounded by what was
        // actually polled: a peer cannot advance the push position — and so
        // unlock compaction — past events it was never sent.
        let acknowledged = response
            .new_cursor
            .sequence
            .min(self.poller.last_sequence());
        self.save_push_cursor(acknowledged)?;

        debug!(count, "Pushed local changes to remote");
        Ok(count)
    }

    /// Converts a WAL event record into a SyncChange, loading the full entity.
    ///
    /// Returns `None` if the entity should be skipped (filtered by config,
    /// or deleted between WAL event and push).
    fn record_to_change(
        &self,
        sequence: u64,
        record: &WatchEventRecord,
    ) -> Result<Option<SyncChange>, SyncError> {
        let collective_id = CollectiveId::from_bytes(record.collective_id);
        let timestamp = Timestamp::from_millis(record.timestamp_ms);

        // Filter by collective if configured
        if let Some(ref allowed) = self.config.collectives {
            if !allowed.contains(&collective_id) {
                trace!(seq = sequence, "Skipping change: collective filtered");
                return Ok(None);
            }
        }

        // Filter by entity type based on config
        match record.entity_type {
            EntityTypeTag::Relation if !self.config.sync_relations => {
                trace!(seq = sequence, "Skipping relation: sync_relations=false");
                return Ok(None);
            }
            EntityTypeTag::Insight if !self.config.sync_insights => {
                trace!(seq = sequence, "Skipping insight: sync_insights=false");
                return Ok(None);
            }
            _ => {}
        }

        let entity_type = match record.entity_type {
            EntityTypeTag::Experience => SyncEntityType::Experience,
            EntityTypeTag::Relation => SyncEntityType::Relation,
            EntityTypeTag::Insight => SyncEntityType::Insight,
            EntityTypeTag::Collective => SyncEntityType::Collective,
        };

        let payload = self.build_payload(record)?;
        let payload = match payload {
            Some(p) => p,
            None => {
                trace!(seq = sequence, "Skipping change: entity no longer exists");
                return Ok(None);
            }
        };

        Ok(Some(SyncChange {
            sequence,
            source_instance: self.local_instance_id,
            collective_id,
            entity_type,
            payload,
            timestamp,
        }))
    }

    /// Builds the SyncPayload by loading the full entity from storage.
    ///
    /// Returns `None` if the entity was deleted between WAL event and now.
    fn build_payload(&self, record: &WatchEventRecord) -> Result<Option<SyncPayload>, SyncError> {
        let map_err = |e: crate::error::PulseDBError| {
            SyncError::transport(format!("Failed to load entity for sync: {}", e))
        };

        match (record.entity_type, record.event_type) {
            // Experience events
            (EntityTypeTag::Experience, WatchEventTypeTag::Created) => {
                let id = ExperienceId::from_bytes(record.entity_id);
                match self.db.get_experience(id).map_err(map_err)? {
                    Some(exp) => Ok(Some(SyncPayload::ExperienceCreated(exp))),
                    None => Ok(None), // Deleted before push
                }
            }
            (EntityTypeTag::Experience, WatchEventTypeTag::Updated) => {
                let id = ExperienceId::from_bytes(record.entity_id);
                match self.db.get_experience(id).map_err(map_err)? {
                    Some(exp) => {
                        // Send all current mutable field values
                        let update = SerializableExperienceUpdate {
                            importance: Some(exp.importance),
                            confidence: Some(exp.confidence),
                            domain: Some(exp.domain.clone()),
                            tags: Some(exp.tags.clone()),
                            related_files: Some(exp.related_files.clone()),
                            archived: Some(exp.archived),
                            applications: Some(exp.applications.clone()),
                            last_reinforced: Some(exp.last_reinforced),
                        };
                        Ok(Some(SyncPayload::ExperienceUpdated {
                            id,
                            update,
                            timestamp: Timestamp::from_millis(record.timestamp_ms),
                        }))
                    }
                    None => Ok(None),
                }
            }
            (EntityTypeTag::Experience, WatchEventTypeTag::Archived) => {
                let id = ExperienceId::from_bytes(record.entity_id);
                Ok(Some(SyncPayload::ExperienceArchived {
                    id,
                    timestamp: Timestamp::from_millis(record.timestamp_ms),
                }))
            }
            (EntityTypeTag::Experience, WatchEventTypeTag::Deleted) => {
                let id = ExperienceId::from_bytes(record.entity_id);
                Ok(Some(SyncPayload::ExperienceDeleted {
                    id,
                    timestamp: Timestamp::from_millis(record.timestamp_ms),
                }))
            }

            // Relation events
            (EntityTypeTag::Relation, WatchEventTypeTag::Created) => {
                let id = RelationId::from_bytes(record.entity_id);
                match self.db.get_relation(id).map_err(map_err)? {
                    Some(rel) => Ok(Some(SyncPayload::RelationCreated(rel))),
                    None => Ok(None),
                }
            }
            (EntityTypeTag::Relation, WatchEventTypeTag::Deleted) => {
                let id = RelationId::from_bytes(record.entity_id);
                Ok(Some(SyncPayload::RelationDeleted {
                    id,
                    timestamp: Timestamp::from_millis(record.timestamp_ms),
                }))
            }

            // Insight events
            (EntityTypeTag::Insight, WatchEventTypeTag::Created) => {
                let id = InsightId::from_bytes(record.entity_id);
                match self.db.get_insight(id).map_err(map_err)? {
                    Some(insight) => Ok(Some(SyncPayload::InsightCreated(insight))),
                    None => Ok(None),
                }
            }
            (EntityTypeTag::Insight, WatchEventTypeTag::Deleted) => {
                let id = InsightId::from_bytes(record.entity_id);
                Ok(Some(SyncPayload::InsightDeleted {
                    id,
                    timestamp: Timestamp::from_millis(record.timestamp_ms),
                }))
            }

            // Collective events
            (EntityTypeTag::Collective, WatchEventTypeTag::Created) => {
                let id = CollectiveId::from_bytes(record.entity_id);
                match self.db.get_collective(id).map_err(map_err)? {
                    Some(collective) => Ok(Some(SyncPayload::CollectiveCreated(collective))),
                    None => Ok(None),
                }
            }

            // Unexpected combinations (e.g., Collective + Deleted) — skip
            (entity_type, event_type) => {
                warn!(
                    ?entity_type,
                    ?event_type,
                    "Unexpected WAL event combination, skipping"
                );
                Ok(None)
            }
        }
    }

    /// Persists the push position for this peer (push side only — the pull
    /// position is owned by the manager's pull path and is never touched here).
    fn save_push_cursor(&self, sequence: u64) -> Result<(), SyncError> {
        self.db
            .storage_for_test()
            .update_push_cursor(&self.peer_instance_id, sequence)
            .map_err(|e| SyncError::transport(format!("Failed to save push cursor: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tempfile::tempdir;

    use super::*;
    use crate::sync::transport_mem::InMemorySyncTransport;
    use crate::{Config, ExperienceType, NewExperience, PulseDB};

    /// `SyncConfig::batch_size` is documented as "maximum number of changes per
    /// sync batch", and `SyncConfig::validate` sizes `max_request_bytes` from
    /// it. Both are claims about the push path, so the push path's poller must
    /// be built with it: on [`ChangePoller`]'s own default (1000, for non-sync
    /// callers) a configured `batch_size` of 2 would still push everything the
    /// WAL had.
    #[tokio::test]
    async fn a_push_cycle_sends_at_most_batch_size_changes() {
        let dir = tempdir().unwrap();
        let db =
            Arc::new(PulseDB::open(dir.path().join("push-batch.db"), Config::default()).unwrap());

        // WAL event 1 is the collective; 2..=6 are the experiences.
        let cid = db.create_collective("push-batch-bound").unwrap();
        for _ in 0..5 {
            db.record_experience(NewExperience {
                collective_id: cid,
                content: "pusher batch bound".to_string(),
                experience_type: ExperienceType::Generic { category: None },
                embedding: Some(vec![0.1f32; 384]),
                importance: 0.5,
                ..Default::default()
            })
            .unwrap();
        }

        let (transport, _peer) = InMemorySyncTransport::new_pair();
        let peer_instance_id = transport.instance_id();
        let mut pusher = LocalChangePusher::new(
            Arc::clone(&db),
            Arc::new(transport),
            SyncConfig {
                batch_size: 2,
                ..SyncConfig::default()
            },
            db.instance_id(),
            peer_instance_id,
            0,
        );

        for cycle in 0..3 {
            assert_eq!(
                pusher.push_pending().await.unwrap(),
                2,
                "cycle {cycle} pushed more than the configured batch_size"
            );
        }
        assert_eq!(
            pusher.push_pending().await.unwrap(),
            0,
            "six WAL events in three batches of two leaves nothing pending"
        );
    }
}
