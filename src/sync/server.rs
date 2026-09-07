//! Server-side sync handler for HTTP consumers.
//!
//! [`SyncServer`] provides framework-agnostic methods for handling sync
//! requests. Consumers wire these into their web framework (Axum, Actix, etc.)
//! without PulseDB taking a dependency on any web framework.
//!
//! # Example (Axum)
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use axum::{Router, routing::{get, post}, extract::State, body::Bytes, http::StatusCode};
//! use pulsedb::sync::server::SyncServer;
//!
//! async fn handle_health(State(server): State<Arc<SyncServer>>) -> StatusCode {
//!     match server.handle_health() {
//!         Ok(()) => StatusCode::OK,
//!         Err(_) => StatusCode::SERVICE_UNAVAILABLE,
//!     }
//! }
//!
//! async fn handle_handshake(State(server): State<Arc<SyncServer>>, body: Bytes) -> Result<Vec<u8>, StatusCode> {
//!     server.handle_handshake_bytes(&body).map_err(|e| {
//!         if e.is_payload_too_large() {
//!             StatusCode::PAYLOAD_TOO_LARGE
//!         } else {
//!             StatusCode::BAD_REQUEST
//!         }
//!     })
//! }
//! ```
//!
//! # Request byte cap
//!
//! Every `handle_*_bytes` method compares the raw body length against
//! [`SyncConfig::max_request_bytes`] **before** reading the wire preamble or
//! handing the body to postcard, and refuses an oversized body with the typed
//! [`SyncError::PayloadTooLarge`]. PulseDB builds no router, so this is the
//! cap it owns at the network edge (ADR-009); the consumer's framework body
//! limit stacks on top of it.

use std::sync::{Arc, Mutex};

use tracing::{debug, info, instrument, warn};

use crate::db::PulseDB;
use crate::watch::ChangePoller;

use super::applier::RemoteChangeApplier;
use super::config::SyncConfig;
use super::error::SyncError;
use super::types::{
    HandshakeRequest, HandshakeResponse, InstanceId, PullRequest, PullResponse, PushResponse,
    SyncChange, SyncPosition, SyncStats,
};
use super::{read_wire_preamble, write_wire_preamble, SYNC_PROTOCOL_VERSION};

/// Server-side sync handler.
///
/// Processes incoming sync requests from remote peers. Framework-agnostic —
/// consumers create web handlers that delegate to this struct's methods.
///
/// The server manages its own `ChangePoller` for serving pull requests and
/// delegates push handling to `RemoteChangeApplier`.
pub struct SyncServer {
    db: Arc<PulseDB>,
    instance_id: InstanceId,
    config: SyncConfig,
    stats: Mutex<SyncStats>,
}

impl SyncServer {
    /// Creates a new SyncServer for the given database.
    pub fn new(db: Arc<PulseDB>, config: SyncConfig) -> Self {
        // Read once, here — same pre-construction rule as `SyncManager`.
        let instance_id = db.instance_id();
        Self {
            db,
            instance_id,
            config,
            stats: Mutex::new(SyncStats::default()),
        }
    }

    /// Returns the server's instance ID.
    pub fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    /// Returns a snapshot of the local-only sync counters accumulated over
    /// every change pushed to this server.
    ///
    /// See [`SyncStats::skewed_timestamps`] for the #13 skew counter. These
    /// counters never travel on the wire — `PushResponse` is unchanged.
    pub fn stats(&self) -> SyncStats {
        *self.stats.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ─── High-level handlers (typed) ─────────────────────────────────

    /// Handles a handshake request.
    #[instrument(skip(self, request), fields(peer = %request.instance_id))]
    pub fn handle_handshake(
        &self,
        request: HandshakeRequest,
    ) -> Result<HandshakeResponse, SyncError> {
        if request.protocol_version != SYNC_PROTOCOL_VERSION {
            return Ok(HandshakeResponse {
                instance_id: self.instance_id,
                protocol_version: SYNC_PROTOCOL_VERSION,
                accepted: false,
                reason: Some(format!(
                    "Protocol version mismatch: server v{}, client v{}",
                    SYNC_PROTOCOL_VERSION, request.protocol_version
                )),
            });
        }

        info!(peer = %request.instance_id, "Sync handshake accepted");
        Ok(HandshakeResponse {
            instance_id: self.instance_id,
            protocol_version: SYNC_PROTOCOL_VERSION,
            accepted: true,
            reason: None,
        })
    }

    /// Handles a push request — applies remote changes locally.
    #[instrument(skip(self, changes), fields(count = changes.len()))]
    /// The acknowledged position is the highest sequence the applier handled
    /// safely, **not** the highest sequence received: the sender turns this
    /// into its `push_sequence`, and `compact_wal` deletes below it, so
    /// acknowledging a change that failed to apply would let the sender
    /// discard a WAL event this peer never stored.
    pub fn handle_push(&self, changes: Vec<SyncChange>) -> Result<PushResponse, SyncError> {
        let source = changes
            .first()
            .map(|c| c.source_instance)
            .unwrap_or_else(InstanceId::nil);

        let applier = RemoteChangeApplier::new(Arc::clone(&self.db), self.config.clone());
        let result = applier.apply_batch(changes)?;
        result.record_into(&self.stats);

        debug!(
            accepted = result.applied,
            skipped = result.skipped,
            conflicts = result.conflicts,
            skewed_timestamps = result.skewed_timestamps,
            "Handled push"
        );

        Ok(PushResponse {
            accepted: result.applied,
            rejected: result.skipped,
            new_cursor: SyncPosition::new(source, result.safe_through.unwrap_or(0)),
        })
    }

    /// Handles a pull request — serves local changes to the remote peer.
    #[instrument(skip(self, request))]
    pub fn handle_pull(&self, request: PullRequest) -> Result<PullResponse, SyncError> {
        let storage = self.db.storage_for_test();
        let mut poller = ChangePoller::from_sequence(request.cursor.sequence);

        let events = poller
            .poll_sync_events(storage)
            .map_err(|e| SyncError::transport(format!("Failed to poll WAL for pull: {}", e)))?;

        // Build SyncChanges from WAL events (same logic as pusher)
        let mut changes = Vec::new();
        for (sequence, record) in &events {
            if let Some(change) =
                build_change_from_record(&self.db, *sequence, record, self.instance_id)?
            {
                // Apply collective filter
                if let Some(ref allowed) = request.collectives {
                    if !allowed.contains(&change.collective_id) {
                        continue;
                    }
                }
                changes.push(change);
                if changes.len() >= request.batch_size {
                    break;
                }
            }
        }

        let has_more = events.len() > changes.len();
        let new_seq = changes
            .last()
            .map(|c| c.sequence)
            .unwrap_or(request.cursor.sequence);

        Ok(PullResponse {
            changes,
            has_more,
            new_cursor: SyncPosition::new(self.instance_id, new_seq),
        })
    }

    /// Handles a health check.
    pub fn handle_health(&self) -> Result<(), SyncError> {
        // Verify DB is accessible by reading metadata
        let _seq = self
            .db
            .get_current_sequence()
            .map_err(|e| SyncError::transport(format!("Health check failed: {}", e)))?;
        Ok(())
    }

    // ─── Byte-level handlers (postcard in/out for HTTP) ──────────────

    /// Refuses a body longer than [`SyncConfig::max_request_bytes`].
    ///
    /// This is the first thing every byte-level handler does — a pure
    /// `len()` comparison, so an oversized body costs nothing beyond the bytes
    /// the framework already buffered and never reaches the preamble read or
    /// the postcard decoder.
    fn check_request_size(&self, body: &[u8]) -> Result<(), SyncError> {
        let max = self.config.max_request_bytes;
        if body.len() > max {
            warn!(
                size = body.len(),
                max_request_bytes = max,
                "Refusing oversized sync request before decode"
            );
            return Err(SyncError::PayloadTooLarge {
                size: body.len(),
                max,
            });
        }
        Ok(())
    }

    /// Handles a handshake from raw wire bytes.
    ///
    /// The body length is checked against `max_request_bytes` first. The
    /// handshake is then the ONLY body that carries the serializer-independent
    /// wire preamble. The preamble is parsed by **raw byte-slicing of
    /// `body[..3]` BEFORE** the body is deserialized (see
    /// [`read_wire_preamble`]), so a serializer/version mismatch surfaces as a
    /// typed [`SyncError::WireFormatMismatch`] — not a generic decode error,
    /// and not the soft in-band `accepted: false` path. The response is framed
    /// with the same preamble so the *client* can fail loud on the way back too.
    pub fn handle_handshake_bytes(&self, body: &[u8]) -> Result<Vec<u8>, SyncError> {
        // 0. Byte cap on the raw body — before the preamble, before any decode.
        self.check_request_size(body)?;
        // 1. Validate the preamble by raw byte-slice BEFORE any deserialize.
        let payload = read_wire_preamble(body)?;
        // 2. Only now is it safe to deserialize the framed body.
        let request: HandshakeRequest = postcard::from_bytes(payload).map_err(SyncError::from)?;
        let response = self.handle_handshake(request)?;
        let encoded = postcard::to_allocvec(&response)
            .map_err(|e| SyncError::serialization(e.to_string()))?;
        // 3. Frame the response with the preamble (both directions fail loud).
        Ok(write_wire_preamble(&encoded))
    }

    /// Handles a push from raw postcard bytes.
    ///
    /// The body length is checked against `max_request_bytes` first. Push
    /// bodies carry NO preamble — they are reached only after a successful
    /// handshake pinned the wire version, so a straight postcard decode is safe.
    pub fn handle_push_bytes(&self, body: &[u8]) -> Result<Vec<u8>, SyncError> {
        self.check_request_size(body)?;
        let changes: Vec<SyncChange> = postcard::from_bytes(body).map_err(SyncError::from)?;
        let response = self.handle_push(changes)?;
        postcard::to_allocvec(&response).map_err(|e| SyncError::serialization(e.to_string()))
    }

    /// Handles a pull from raw postcard bytes.
    ///
    /// The body length is checked against `max_request_bytes` first. Pull
    /// bodies carry NO preamble (same rationale as push).
    pub fn handle_pull_bytes(&self, body: &[u8]) -> Result<Vec<u8>, SyncError> {
        self.check_request_size(body)?;
        let request: PullRequest = postcard::from_bytes(body).map_err(SyncError::from)?;
        let response = self.handle_pull(request)?;
        postcard::to_allocvec(&response).map_err(|e| SyncError::serialization(e.to_string()))
    }
}

/// Build a SyncChange from a WAL record by loading the full entity.
fn build_change_from_record(
    db: &PulseDB,
    sequence: u64,
    record: &crate::storage::schema::WatchEventRecord,
    source_instance: InstanceId,
) -> Result<Option<SyncChange>, SyncError> {
    use super::types::{SerializableExperienceUpdate, SyncEntityType, SyncPayload};
    use crate::storage::schema::{EntityTypeTag, WatchEventTypeTag};
    use crate::types::{CollectiveId, ExperienceId, InsightId, RelationId, Timestamp};

    let collective_id = CollectiveId::from_bytes(record.collective_id);
    let timestamp = Timestamp::from_millis(record.timestamp_ms);
    let map_err = |e: crate::error::PulseDBError| {
        SyncError::transport(format!("Failed to load entity: {}", e))
    };

    let entity_type = match record.entity_type {
        EntityTypeTag::Experience => SyncEntityType::Experience,
        EntityTypeTag::Relation => SyncEntityType::Relation,
        EntityTypeTag::Insight => SyncEntityType::Insight,
        EntityTypeTag::Collective => SyncEntityType::Collective,
    };

    let payload = match (record.entity_type, record.event_type) {
        (EntityTypeTag::Experience, WatchEventTypeTag::Created) => {
            let id = ExperienceId::from_bytes(record.entity_id);
            db.get_experience(id)
                .map_err(map_err)?
                .map(SyncPayload::ExperienceCreated)
        }
        (EntityTypeTag::Experience, WatchEventTypeTag::Updated) => {
            let id = ExperienceId::from_bytes(record.entity_id);
            db.get_experience(id)
                .map_err(map_err)?
                .map(|exp| SyncPayload::ExperienceUpdated {
                    id,
                    update: SerializableExperienceUpdate {
                        importance: Some(exp.importance),
                        confidence: Some(exp.confidence),
                        domain: Some(exp.domain.clone()),
                        tags: Some(exp.tags.clone()),
                        related_files: Some(exp.related_files.clone()),
                        archived: Some(exp.archived),
                        applications: Some(exp.applications.clone()),
                        last_reinforced: Some(exp.last_reinforced),
                    },
                    timestamp,
                })
        }
        (EntityTypeTag::Experience, WatchEventTypeTag::Archived) => {
            let id = ExperienceId::from_bytes(record.entity_id);
            Some(SyncPayload::ExperienceArchived { id, timestamp })
        }
        (EntityTypeTag::Experience, WatchEventTypeTag::Deleted) => {
            let id = ExperienceId::from_bytes(record.entity_id);
            Some(SyncPayload::ExperienceDeleted { id, timestamp })
        }
        (EntityTypeTag::Relation, WatchEventTypeTag::Created) => {
            let id = RelationId::from_bytes(record.entity_id);
            db.get_relation(id)
                .map_err(map_err)?
                .map(SyncPayload::RelationCreated)
        }
        (EntityTypeTag::Relation, WatchEventTypeTag::Deleted) => {
            let id = RelationId::from_bytes(record.entity_id);
            Some(SyncPayload::RelationDeleted { id, timestamp })
        }
        (EntityTypeTag::Insight, WatchEventTypeTag::Created) => {
            let id = InsightId::from_bytes(record.entity_id);
            db.get_insight(id)
                .map_err(map_err)?
                .map(SyncPayload::InsightCreated)
        }
        (EntityTypeTag::Insight, WatchEventTypeTag::Deleted) => {
            let id = InsightId::from_bytes(record.entity_id);
            Some(SyncPayload::InsightDeleted { id, timestamp })
        }
        (EntityTypeTag::Collective, WatchEventTypeTag::Created) => {
            let id = CollectiveId::from_bytes(record.entity_id);
            db.get_collective(id)
                .map_err(map_err)?
                .map(SyncPayload::CollectiveCreated)
        }
        _ => None,
    };

    Ok(payload.map(|p| SyncChange {
        sequence,
        source_instance,
        collective_id,
        entity_type,
        payload: p,
        timestamp,
    }))
}

// SyncServer is Send + Sync (Arc<PulseDB> is Send + Sync)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_server_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SyncServer>();
    }
}
