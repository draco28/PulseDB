//! Pluggable transport trait for the sync protocol.
//!
//! Consumers choose their transport: in-memory for testing,
//! HTTP for production, WebSocket for real-time sync.
//! PulseDB provides the trait; consumers (or feature-gated modules)
//! provide implementations.

use async_trait::async_trait;

use super::error::SyncError;
use super::types::{
    HandshakeRequest, HandshakeResponse, PullRequest, PullResponse, PushResponse, SyncChange,
};

/// Transport layer for the sync protocol.
///
/// Implementations handle the wire protocol for exchanging sync data
/// between PulseDB instances. The sync engine calls these methods;
/// the transport handles serialization, networking, and authentication.
///
/// # Implementations
///
/// - [`super::transport_mem::InMemorySyncTransport`] — In-memory for testing
/// - `HttpSyncTransport` — HTTP/HTTPS (behind `sync-http` feature, Phase 4)
/// - `WebSocketSyncTransport` — WebSocket (behind `sync-websocket` feature, Phase 4)
///
/// # Example
///
/// ```rust
/// use pulsedb::sync::transport::SyncTransport;
/// use pulsedb::sync::transport_mem::InMemorySyncTransport;
///
/// let (local, remote) = InMemorySyncTransport::new_pair();
/// // local and remote can now exchange sync data via shared buffer
/// ```
#[async_trait]
pub trait SyncTransport: Send + Sync {
    /// Perform a handshake with the remote peer.
    ///
    /// Called once when establishing a sync connection. Exchanges
    /// instance IDs, protocol versions, and capabilities.
    async fn handshake(&self, request: HandshakeRequest) -> Result<HandshakeResponse, SyncError>;

    /// Push local changes to the remote peer.
    ///
    /// The transport sends the changes and returns how many were
    /// accepted/rejected by the remote.
    async fn push_changes(&self, changes: Vec<SyncChange>) -> Result<PushResponse, SyncError>;

    /// Pull changes from the remote peer.
    ///
    /// Requests changes starting from the cursor position, up to
    /// the specified batch size.
    async fn pull_changes(&self, request: PullRequest) -> Result<PullResponse, SyncError>;

    /// Check if the remote peer is reachable.
    ///
    /// Returns `Ok(())` if the remote is healthy, or a `SyncError`
    /// describing the connectivity issue.
    async fn health_check(&self) -> Result<(), SyncError>;
}
