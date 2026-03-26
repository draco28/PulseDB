//! In-memory sync transport for testing.
//!
//! [`InMemorySyncTransport`] simulates network sync between two PulseDB
//! instances using a shared in-memory buffer. Use [`InMemorySyncTransport::new_pair()`]
//! to create two transports that share the same change buffer.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::error::SyncError;
use super::transport::SyncTransport;
use super::types::{
    HandshakeRequest, HandshakeResponse, InstanceId, PullRequest, PullResponse, PushResponse,
    SyncChange, SyncCursor,
};
use super::SYNC_PROTOCOL_VERSION;

/// Shared state between paired in-memory transports.
#[derive(Debug)]
struct SharedBuffer {
    changes: Vec<SyncChange>,
}

/// In-memory transport for testing sync without network I/O.
///
/// Two instances created via [`new_pair()`](Self::new_pair) share an
/// in-memory buffer. Push appends to the buffer; pull reads from it.
///
/// # Example
///
/// ```rust
/// use pulsedb::sync::transport_mem::InMemorySyncTransport;
///
/// let (local, remote) = InMemorySyncTransport::new_pair();
/// ```
#[derive(Debug, Clone)]
pub struct InMemorySyncTransport {
    /// The peer instance ID this transport represents.
    peer_instance_id: InstanceId,
    /// Shared change buffer between paired transports.
    buffer: Arc<Mutex<SharedBuffer>>,
}

impl InMemorySyncTransport {
    /// Creates a pair of connected in-memory transports.
    ///
    /// Both transports share the same change buffer, simulating
    /// a network connection between two PulseDB instances.
    pub fn new_pair() -> (Self, Self) {
        let buffer = Arc::new(Mutex::new(SharedBuffer {
            changes: Vec::new(),
        }));

        let local = Self {
            peer_instance_id: InstanceId::new(),
            buffer: Arc::clone(&buffer),
        };
        let remote = Self {
            peer_instance_id: InstanceId::new(),
            buffer,
        };

        (local, remote)
    }

    /// Returns the instance ID this transport represents.
    pub fn instance_id(&self) -> InstanceId {
        self.peer_instance_id
    }
}

#[async_trait]
impl SyncTransport for InMemorySyncTransport {
    async fn handshake(&self, _request: HandshakeRequest) -> Result<HandshakeResponse, SyncError> {
        Ok(HandshakeResponse {
            instance_id: self.peer_instance_id,
            protocol_version: SYNC_PROTOCOL_VERSION,
            accepted: true,
            reason: None,
        })
    }

    async fn push_changes(&self, changes: Vec<SyncChange>) -> Result<PushResponse, SyncError> {
        let accepted = changes.len();
        let max_seq = changes.iter().map(|c| c.sequence).max().unwrap_or(0);

        let mut buf = self
            .buffer
            .lock()
            .map_err(|e| SyncError::transport(format!("buffer lock poisoned: {}", e)))?;
        buf.changes.extend(changes);

        Ok(PushResponse {
            accepted,
            rejected: 0,
            new_cursor: SyncCursor {
                instance_id: self.peer_instance_id,
                last_sequence: max_seq,
            },
        })
    }

    async fn pull_changes(&self, request: PullRequest) -> Result<PullResponse, SyncError> {
        let buf = self
            .buffer
            .lock()
            .map_err(|e| SyncError::transport(format!("buffer lock poisoned: {}", e)))?;

        let after_seq = request.cursor.last_sequence;
        let batch_size = request.batch_size;

        // Filter changes after the cursor position
        let mut matching: Vec<SyncChange> = buf
            .changes
            .iter()
            .filter(|c| c.sequence > after_seq)
            .filter(|c| {
                // Apply collective filter if specified
                request
                    .collectives
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&c.collective_id))
            })
            .cloned()
            .collect();

        // Sort by sequence for deterministic ordering
        matching.sort_by_key(|c| c.sequence);

        let has_more = matching.len() > batch_size;
        let batch: Vec<SyncChange> = matching.into_iter().take(batch_size).collect();

        let new_seq = batch.last().map_or(after_seq, |c| c.sequence);

        Ok(PullResponse {
            changes: batch,
            has_more,
            new_cursor: SyncCursor {
                instance_id: self.peer_instance_id,
                last_sequence: new_seq,
            },
        })
    }

    async fn health_check(&self) -> Result<(), SyncError> {
        // In-memory transport is always healthy
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::types::{SyncEntityType, SyncPayload, SyncStatus};
    use crate::types::{CollectiveId, Timestamp};

    fn make_test_change(seq: u64, collective_id: CollectiveId) -> SyncChange {
        use crate::collective::Collective;
        SyncChange {
            sequence: seq,
            source_instance: InstanceId::new(),
            collective_id,
            entity_type: SyncEntityType::Collective,
            payload: SyncPayload::CollectiveCreated(Collective {
                id: collective_id,
                name: format!("test-{}", seq),
                owner_id: None,
                embedding_dimension: 384,
                created_at: Timestamp::now(),
                updated_at: Timestamp::now(),
            }),
            timestamp: Timestamp::now(),
        }
    }

    #[tokio::test]
    async fn test_new_pair_creates_distinct_instances() {
        let (local, remote) = InMemorySyncTransport::new_pair();
        assert_ne!(local.instance_id(), remote.instance_id());
    }

    #[tokio::test]
    async fn test_handshake_always_accepts() {
        let (transport, _) = InMemorySyncTransport::new_pair();
        let req = HandshakeRequest {
            instance_id: InstanceId::new(),
            protocol_version: SYNC_PROTOCOL_VERSION,
            capabilities: vec![],
        };
        let resp = transport.handshake(req).await.unwrap();
        assert!(resp.accepted);
        assert_eq!(resp.protocol_version, SYNC_PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn test_health_check_always_ok() {
        let (transport, _) = InMemorySyncTransport::new_pair();
        assert!(transport.health_check().await.is_ok());
    }

    #[tokio::test]
    async fn test_push_then_pull() {
        let (local, remote) = InMemorySyncTransport::new_pair();
        let cid = CollectiveId::new();

        // Push 3 changes via local
        let changes = vec![
            make_test_change(1, cid),
            make_test_change(2, cid),
            make_test_change(3, cid),
        ];
        let push_resp = local.push_changes(changes).await.unwrap();
        assert_eq!(push_resp.accepted, 3);
        assert_eq!(push_resp.rejected, 0);

        // Pull all via remote (shared buffer)
        let pull_req = PullRequest {
            cursor: SyncCursor::new(remote.instance_id()),
            batch_size: 100,
            collectives: None,
        };
        let pull_resp = remote.pull_changes(pull_req).await.unwrap();
        assert_eq!(pull_resp.changes.len(), 3);
        assert!(!pull_resp.has_more);
        assert_eq!(pull_resp.new_cursor.last_sequence, 3);
    }

    #[tokio::test]
    async fn test_pull_respects_cursor() {
        let (local, remote) = InMemorySyncTransport::new_pair();
        let cid = CollectiveId::new();

        // Push 5 changes
        let changes: Vec<SyncChange> = (1..=5).map(|seq| make_test_change(seq, cid)).collect();
        local.push_changes(changes).await.unwrap();

        // Pull starting after sequence 3
        let pull_req = PullRequest {
            cursor: SyncCursor {
                instance_id: remote.instance_id(),
                last_sequence: 3,
            },
            batch_size: 100,
            collectives: None,
        };
        let pull_resp = remote.pull_changes(pull_req).await.unwrap();
        assert_eq!(pull_resp.changes.len(), 2);
        assert_eq!(pull_resp.changes[0].sequence, 4);
        assert_eq!(pull_resp.changes[1].sequence, 5);
    }

    #[tokio::test]
    async fn test_pull_respects_batch_size() {
        let (local, remote) = InMemorySyncTransport::new_pair();
        let cid = CollectiveId::new();

        // Push 10 changes
        let changes: Vec<SyncChange> = (1..=10).map(|seq| make_test_change(seq, cid)).collect();
        local.push_changes(changes).await.unwrap();

        // Pull with batch_size=3
        let pull_req = PullRequest {
            cursor: SyncCursor::new(remote.instance_id()),
            batch_size: 3,
            collectives: None,
        };
        let pull_resp = remote.pull_changes(pull_req).await.unwrap();
        assert_eq!(pull_resp.changes.len(), 3);
        assert!(pull_resp.has_more);
        assert_eq!(pull_resp.new_cursor.last_sequence, 3);
    }

    #[tokio::test]
    async fn test_pull_filters_by_collective() {
        let (local, remote) = InMemorySyncTransport::new_pair();
        let cid_a = CollectiveId::new();
        let cid_b = CollectiveId::new();

        let changes = vec![
            make_test_change(1, cid_a),
            make_test_change(2, cid_b),
            make_test_change(3, cid_a),
        ];
        local.push_changes(changes).await.unwrap();

        // Pull only cid_a
        let pull_req = PullRequest {
            cursor: SyncCursor::new(remote.instance_id()),
            batch_size: 100,
            collectives: Some(vec![cid_a]),
        };
        let pull_resp = remote.pull_changes(pull_req).await.unwrap();
        assert_eq!(pull_resp.changes.len(), 2);
        assert!(pull_resp.changes.iter().all(|c| c.collective_id == cid_a));
    }

    #[tokio::test]
    async fn test_pull_empty_buffer() {
        let (_, remote) = InMemorySyncTransport::new_pair();
        let pull_req = PullRequest {
            cursor: SyncCursor::new(remote.instance_id()),
            batch_size: 100,
            collectives: None,
        };
        let pull_resp = remote.pull_changes(pull_req).await.unwrap();
        assert!(pull_resp.changes.is_empty());
        assert!(!pull_resp.has_more);
    }

    #[test]
    fn test_sync_status_not_used_here_but_compiles() {
        // Just verify SyncStatus is accessible and works
        let status = SyncStatus::Idle;
        assert_eq!(status, SyncStatus::Idle);
    }
}
