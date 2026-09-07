//! Core types for the PulseDB sync protocol.
//!
//! This module defines the wire types used for synchronizing data between
//! PulseDB instances: change payloads, cursors, handshake messages, and
//! the instance identity type.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::collective::Collective;
use crate::experience::Experience;
use crate::insight::DerivedInsight;
use crate::relation::ExperienceRelation;
use crate::types::{CollectiveId, ExperienceId, InsightId, RelationId, Timestamp};

pub use crate::types::InstanceId;

// ============================================================================
// SyncCursor — Persisted per-peer sync positions (storage record, schema v5)
// ============================================================================

/// Persisted sync positions for a specific peer instance.
///
/// Push and pull are tracked **separately** (issue #9): `push_sequence` is the
/// *local* WAL position the peer has acknowledged receiving, and
/// `pull_sequence` is the *remote* WAL position this instance has applied from
/// the peer. The two live in different sequence spaces and must never
/// overwrite each other — [`PulseDB::compact_wal`](crate::PulseDB::compact_wal)
/// trusts only `push_sequence`.
///
/// This is the **storage** record (postcard-encoded in the `sync_cursors`
/// table; schema v5 split the pre-0.8.0 single `last_sequence`). It is not a
/// wire type: pull and push messages carry a single-direction
/// [`SyncPosition`] so the protocol v4 bytes are unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCursor {
    /// The peer instance this cursor tracks.
    pub instance_id: InstanceId,

    /// Highest **local** WAL sequence the peer has acknowledged (push side).
    ///
    /// WAL compaction never deletes above the minimum of this value over all
    /// known peers; `0` means "nothing pushed yet" and blocks compaction.
    pub push_sequence: u64,

    /// Highest **remote** WAL sequence applied locally from this peer (pull
    /// side). Never feeds compaction.
    pub pull_sequence: u64,
}

impl SyncCursor {
    /// Creates a new cursor with both positions at 0 (beginning of WAL).
    pub fn new(instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            push_sequence: 0,
            pull_sequence: 0,
        }
    }
}

// ============================================================================
// SyncPosition — Single-direction wire position (protocol v4 bytes)
// ============================================================================

/// A single-direction sync position as carried on the wire.
///
/// Rides in [`PullRequest::cursor`], [`PullResponse::new_cursor`] and
/// [`PushResponse::new_cursor`]. `sequence` is the position **for the
/// direction of the message it rides in**: a pull message carries a position
/// in the *remote* WAL (persisted as [`SyncCursor::pull_sequence`]), a push
/// acknowledgement carries a position in the *local* WAL (persisted as
/// [`SyncCursor::push_sequence`]).
///
/// Field order and types are pinned — `{ instance_id: InstanceId, sequence:
/// u64 }` — so the postcard encoding is byte-identical to the pre-0.8.0
/// `SyncCursor` wire shape and `SYNC_PROTOCOL_VERSION` 4 is unchanged. A wire
/// cursor carrying both positions is a protocol v5 change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPosition {
    /// The instance the position refers to.
    pub instance_id: InstanceId,

    /// The WAL sequence for the message's direction.
    pub sequence: u64,
}

impl SyncPosition {
    /// Creates a position at `sequence` for `instance_id`.
    pub fn new(instance_id: InstanceId, sequence: u64) -> Self {
        Self {
            instance_id,
            sequence,
        }
    }
}

// ============================================================================
// SyncEntityType — What kind of entity changed
// ============================================================================

/// Discriminant for the type of entity in a sync change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SyncEntityType {
    /// An experience was created, updated, archived, or deleted.
    Experience = 0,
    /// A relation was created or deleted.
    Relation = 1,
    /// An insight was created or deleted.
    Insight = 2,
    /// A collective was created.
    Collective = 3,
}

// ============================================================================
// SerializableExperienceUpdate — Wire-safe mirror of ExperienceUpdate
// ============================================================================

/// Wire-safe version of [`crate::ExperienceUpdate`] for sync payloads.
///
/// The original `ExperienceUpdate` does not derive `Serialize`/`Deserialize`,
/// so this struct mirrors its fields with full serde support. Use the `From`
/// impls to convert between the two.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SerializableExperienceUpdate {
    /// New importance score (0.0–1.0).
    pub importance: Option<f32>,

    /// New confidence score (0.0–1.0).
    pub confidence: Option<f32>,

    /// Replace domain tags entirely.
    pub domain: Option<Vec<String>>,

    /// Replace key-value tags entirely (VS-4.3.2).
    pub tags: Option<BTreeMap<String, String>>,

    /// Replace related files entirely.
    pub related_files: Option<Vec<String>>,

    /// Set archived status.
    pub archived: Option<bool>,

    /// Full G-counter applications map for CRDT merge.
    pub applications: Option<BTreeMap<InstanceId, u32>>,

    /// Last reinforcement timestamp for max-timestamp merge.
    pub last_reinforced: Option<Timestamp>,
}

impl From<crate::experience::ExperienceUpdate> for SerializableExperienceUpdate {
    fn from(update: crate::experience::ExperienceUpdate) -> Self {
        Self {
            importance: update.importance,
            confidence: update.confidence,
            domain: update.domain,
            tags: update.tags,
            related_files: update.related_files,
            archived: update.archived,
            applications: None,
            last_reinforced: None,
        }
    }
}

impl From<SerializableExperienceUpdate> for crate::experience::ExperienceUpdate {
    fn from(update: SerializableExperienceUpdate) -> Self {
        Self {
            importance: update.importance,
            confidence: update.confidence,
            domain: update.domain,
            tags: update.tags,
            related_files: update.related_files,
            archived: update.archived,
        }
    }
}

// ============================================================================
// SyncPayload — Full data for each mutation type
// ============================================================================

/// The payload of a sync change, containing all data needed to apply
/// the change on the receiving end.
///
/// Uses full payloads (not deltas) so the receiver has everything needed
/// including embeddings for HNSW insertion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SyncPayload {
    /// A new experience was created.
    ExperienceCreated(Experience),

    /// An existing experience was updated.
    ExperienceUpdated {
        /// The experience that was updated.
        id: ExperienceId,
        /// The fields that changed.
        update: SerializableExperienceUpdate,
        /// When the update occurred.
        timestamp: Timestamp,
    },

    /// An experience was soft-deleted (archived).
    ExperienceArchived {
        /// The archived experience.
        id: ExperienceId,
        /// When the archive occurred.
        timestamp: Timestamp,
    },

    /// An experience was permanently deleted.
    ExperienceDeleted {
        /// The deleted experience.
        id: ExperienceId,
        /// When the deletion occurred.
        timestamp: Timestamp,
    },

    /// A new relation was created.
    RelationCreated(ExperienceRelation),

    /// A relation was deleted.
    RelationDeleted {
        /// The deleted relation.
        id: RelationId,
        /// When the deletion occurred.
        timestamp: Timestamp,
    },

    /// A new insight was created.
    InsightCreated(DerivedInsight),

    /// An insight was deleted.
    InsightDeleted {
        /// The deleted insight.
        id: InsightId,
        /// When the deletion occurred.
        timestamp: Timestamp,
    },

    /// A new collective was created.
    CollectiveCreated(Collective),
}

// ============================================================================
// SyncChange — A single change to sync
// ============================================================================

/// A single change event to be synchronized between PulseDB instances.
///
/// Contains the full payload needed to apply the change, plus metadata
/// about the source instance and WAL position.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncChange {
    /// Source WAL sequence number.
    pub sequence: u64,

    /// The instance that produced this change.
    pub source_instance: InstanceId,

    /// Which collective this change belongs to.
    pub collective_id: CollectiveId,

    /// What kind of entity changed.
    pub entity_type: SyncEntityType,

    /// The full change data.
    pub payload: SyncPayload,

    /// When the change occurred.
    pub timestamp: Timestamp,
}

// ============================================================================
// SyncStatus — Current state of the sync system
// ============================================================================

/// Current operational status of the sync system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncStatus {
    /// Sync is idle, waiting for the next poll interval.
    Idle,
    /// Sync is actively transferring data.
    Syncing,
    /// Sync encountered an error.
    Error(String),
    /// Disconnected from the remote peer.
    Disconnected,
}

// ============================================================================
// SyncStats — Local-only counters (never on the wire)
// ============================================================================

/// Local-only counters accumulated over the changes a peer has applied —
/// by a [`SyncManager`](super::SyncManager) on the pull side and by a
/// `SyncServer` on the push side.
///
/// This type is **not** a wire type: it deliberately derives neither
/// `Serialize` nor `Deserialize`, so a new counter never changes the shape of
/// `PushResponse`/`PullResponse` or moves
/// [`SYNC_PROTOCOL_VERSION`](super::SYNC_PROTOCOL_VERSION).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Number of applied changes whose incoming `last_reinforced` lay beyond
    /// `now + SyncConfig::max_clock_skew_ms` (#13).
    ///
    /// Each one is logged at `warn` with the peer, the experience id and the
    /// skew, and then merged **unchanged** — FR-031's max-merge is never
    /// clamped, rejected or re-timestamped (r1 veto fold C2). The bound is
    /// advisory until protocol v5 carries a record-level time reference.
    pub skewed_timestamps: u64,
}

// ============================================================================
// Handshake messages
// ============================================================================

/// Request sent during sync handshake to establish a connection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandshakeRequest {
    /// The local instance ID.
    pub instance_id: InstanceId,
    /// The sync protocol version.
    pub protocol_version: u32,
    /// Capabilities advertised by this instance.
    pub capabilities: Vec<String>,
}

/// Response to a handshake request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandshakeResponse {
    /// The remote instance ID.
    pub instance_id: InstanceId,
    /// The remote's protocol version.
    pub protocol_version: u32,
    /// Whether the handshake was accepted.
    pub accepted: bool,
    /// Reason for rejection, if not accepted.
    pub reason: Option<String>,
}

// ============================================================================
// Pull request/response
// ============================================================================

/// Request to pull changes from a remote peer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullRequest {
    /// The remote-WAL position to pull changes from (the client's persisted
    /// `pull_sequence` for this peer).
    pub cursor: SyncPosition,
    /// Maximum number of changes to return in this batch.
    pub batch_size: usize,
    /// Optional filter: only pull changes for these collectives.
    pub collectives: Option<Vec<CollectiveId>>,
}

/// Response containing pulled changes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullResponse {
    /// The changes since the cursor position.
    pub changes: Vec<SyncChange>,
    /// Whether there are more changes available.
    pub has_more: bool,
    /// The updated remote-WAL position after this batch (the client persists
    /// `new_cursor.sequence` as its `pull_sequence`).
    pub new_cursor: SyncPosition,
}

// ============================================================================
// Push response
// ============================================================================

/// Response after pushing changes to a remote peer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushResponse {
    /// Number of changes accepted by the remote.
    pub accepted: usize,
    /// Number of changes rejected by the remote.
    pub rejected: usize,
    /// The local-WAL position the remote acknowledged (the pusher persists
    /// `new_cursor.sequence` as its `push_sequence`).
    pub new_cursor: SyncPosition,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_id_new_is_unique() {
        let a = InstanceId::new();
        let b = InstanceId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn test_instance_id_nil() {
        let id = InstanceId::nil();
        assert_eq!(id, InstanceId::default());
        assert_eq!(id, InstanceId::nil());
    }

    #[test]
    fn test_instance_id_bytes_roundtrip() {
        let id = InstanceId::new();
        let bytes = *id.as_bytes();
        let restored = InstanceId::from_bytes(bytes);
        assert_eq!(id, restored);
    }

    #[test]
    fn test_instance_id_display() {
        let id = InstanceId::nil();
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn test_instance_id_postcard_roundtrip() {
        let id = InstanceId::new();
        let bytes = postcard::to_allocvec(&id).unwrap();
        let restored: InstanceId = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(id, restored);
    }

    #[test]
    fn test_sync_cursor_new() {
        let id = InstanceId::new();
        let cursor = SyncCursor::new(id);
        assert_eq!(cursor.instance_id, id);
        assert_eq!(cursor.push_sequence, 0);
        assert_eq!(cursor.pull_sequence, 0);
    }

    #[test]
    fn test_sync_cursor_postcard_roundtrip() {
        let cursor = SyncCursor {
            instance_id: InstanceId::new(),
            push_sequence: 42,
            pull_sequence: 7,
        };
        let bytes = postcard::to_allocvec(&cursor).unwrap();
        let restored: SyncCursor = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(cursor, restored);
    }

    /// r1.s1.w1: `SyncPosition` must encode byte-for-byte like the 0.7.0 wire
    /// `SyncCursor { instance_id, last_sequence }`, so protocol v4 bytes are
    /// unchanged. Pinned literally against postcard's encoding: a uuid is
    /// `varint(16) ‖ 16 raw bytes` and the sequence a LEB128 varint.
    #[test]
    fn sync_position_wire_bytes_match_v4_cursor_encoding() {
        let id_bytes = [0x5A; 16];
        let position = SyncPosition::new(InstanceId::from_bytes(id_bytes), 7);
        let mut expected = vec![0x10];
        expected.extend_from_slice(&id_bytes);
        expected.push(0x07);
        assert_eq!(postcard::to_stdvec(&position).unwrap(), expected);
        // A 0.7.0-encoded cursor decodes as a SyncPosition.
        let decoded: SyncPosition = postcard::from_bytes(&expected).unwrap();
        assert_eq!(decoded, position);
        // Multi-byte varint sequence (300 = 0xAC 0x02): no fixed-width assumption.
        let big = SyncPosition::new(InstanceId::from_bytes(id_bytes), 300);
        let mut expected_big = vec![0x10];
        expected_big.extend_from_slice(&id_bytes);
        expected_big.extend_from_slice(&[0xAC, 0x02]);
        assert_eq!(postcard::to_stdvec(&big).unwrap(), expected_big);
    }

    #[test]
    fn test_sync_entity_type_repr() {
        assert_eq!(SyncEntityType::Experience as u8, 0);
        assert_eq!(SyncEntityType::Relation as u8, 1);
        assert_eq!(SyncEntityType::Insight as u8, 2);
        assert_eq!(SyncEntityType::Collective as u8, 3);
    }

    #[test]
    fn test_serializable_experience_update_from_conversion() {
        let update = crate::experience::ExperienceUpdate {
            importance: Some(0.9),
            confidence: None,
            domain: Some(vec!["rust".to_string()]),
            tags: None,
            related_files: None,
            archived: Some(false),
        };
        let serializable: SerializableExperienceUpdate = update.into();
        assert_eq!(serializable.importance, Some(0.9));
        assert_eq!(serializable.confidence, None);
        assert_eq!(serializable.domain, Some(vec!["rust".to_string()]));
        assert_eq!(serializable.archived, Some(false));
    }

    #[test]
    fn test_serializable_experience_update_into_conversion() {
        let serializable = SerializableExperienceUpdate {
            importance: Some(0.5),
            confidence: Some(0.8),
            domain: None,
            tags: None,
            related_files: Some(vec!["main.rs".to_string()]),
            archived: None,
            applications: None,
            last_reinforced: None,
        };
        let update: crate::experience::ExperienceUpdate = serializable.into();
        assert_eq!(update.importance, Some(0.5));
        assert_eq!(update.confidence, Some(0.8));
        assert_eq!(update.related_files, Some(vec!["main.rs".to_string()]));
    }

    #[test]
    fn test_serializable_experience_update_postcard_roundtrip() {
        let update = SerializableExperienceUpdate {
            importance: Some(0.7),
            confidence: Some(0.9),
            domain: Some(vec!["test".to_string()]),
            tags: Some(BTreeMap::from([(
                "entity.type".to_string(),
                "person".to_string(),
            )])),
            related_files: None,
            archived: Some(true),
            applications: Some(std::collections::BTreeMap::from([(InstanceId::new(), 2)])),
            last_reinforced: Some(Timestamp::now()),
        };
        let bytes = postcard::to_allocvec(&update).unwrap();
        let restored: SerializableExperienceUpdate = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(update.importance, restored.importance);
        assert_eq!(update.confidence, restored.confidence);
        assert_eq!(update.domain, restored.domain);
        assert_eq!(update.archived, restored.archived);
        assert_eq!(update.applications, restored.applications);
        assert_eq!(update.last_reinforced, restored.last_reinforced);
    }

    #[test]
    fn test_sync_stats_default_is_zero() {
        let stats = SyncStats::default();
        assert_eq!(stats.skewed_timestamps, 0);
        assert_eq!(
            stats,
            SyncStats {
                skewed_timestamps: 0
            }
        );
    }

    #[test]
    fn test_sync_status_equality() {
        assert_eq!(SyncStatus::Idle, SyncStatus::Idle);
        assert_eq!(SyncStatus::Error("x".into()), SyncStatus::Error("x".into()));
        assert_ne!(SyncStatus::Idle, SyncStatus::Syncing);
    }

    #[test]
    fn test_handshake_request_postcard_roundtrip() {
        let req = HandshakeRequest {
            instance_id: InstanceId::new(),
            protocol_version: crate::sync::SYNC_PROTOCOL_VERSION,
            capabilities: vec!["push".to_string(), "pull".to_string()],
        };
        let bytes = postcard::to_allocvec(&req).unwrap();
        let restored: HandshakeRequest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(req.instance_id, restored.instance_id);
        assert_eq!(req.protocol_version, restored.protocol_version);
        assert_eq!(req.capabilities, restored.capabilities);
    }

    #[test]
    fn test_pull_request_postcard_roundtrip() {
        let req = PullRequest {
            cursor: SyncPosition::new(InstanceId::new(), 0),
            batch_size: 500,
            collectives: Some(vec![CollectiveId::new()]),
        };
        let bytes = postcard::to_allocvec(&req).unwrap();
        let restored: PullRequest = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(req.cursor, restored.cursor);
        assert_eq!(req.batch_size, restored.batch_size);
    }

    #[test]
    fn test_push_response_postcard_roundtrip() {
        let resp = PushResponse {
            accepted: 10,
            rejected: 2,
            new_cursor: SyncPosition::new(InstanceId::new(), 100),
        };
        let bytes = postcard::to_allocvec(&resp).unwrap();
        let restored: PushResponse = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(resp.accepted, restored.accepted);
        assert_eq!(resp.rejected, restored.rejected);
        assert_eq!(resp.new_cursor, restored.new_cursor);
    }
}
