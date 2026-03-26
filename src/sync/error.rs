//! Sync-specific error types.
//!
//! [`SyncError`] covers all failure modes in the sync protocol:
//! transport failures, handshake rejections, serialization issues,
//! and shutdown coordination.

use thiserror::Error;

use super::types::InstanceId;

/// Errors specific to the sync protocol.
///
/// These are wrapped into [`crate::PulseDBError::Sync`] when propagated
/// through the public API.
#[derive(Debug, Error)]
pub enum SyncError {
    /// Transport-level failure (network I/O, connection refused, etc.).
    #[error("Sync transport error: {0}")]
    Transport(String),

    /// Handshake was rejected by the remote peer.
    #[error("Sync handshake failed: {0}")]
    Handshake(String),

    /// Failed to serialize or deserialize a sync message.
    #[error("Sync serialization error: {0}")]
    Serialization(String),

    /// Operation timed out waiting for a response.
    #[error("Sync operation timed out")]
    Timeout,

    /// Connection to the remote peer was lost.
    #[error("Connection to sync peer lost")]
    ConnectionLost,

    /// Protocol version mismatch between peers.
    #[error("Sync protocol version mismatch: local v{local}, remote v{remote}")]
    ProtocolVersion {
        /// Local protocol version.
        local: u32,
        /// Remote protocol version.
        remote: u32,
    },

    /// Received an invalid or unrecognized payload.
    #[error("Invalid sync payload: {0}")]
    InvalidPayload(String),

    /// No cursor found for the specified peer instance.
    #[error("No sync cursor found for instance {instance}")]
    CursorNotFound {
        /// The peer instance whose cursor was not found.
        instance: InstanceId,
    },

    /// The sync system is shutting down.
    #[error("Sync system is shutting down")]
    Shutdown,
}

impl SyncError {
    /// Creates a transport error with the given message.
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport(msg.into())
    }

    /// Creates a handshake error with the given message.
    pub fn handshake(msg: impl Into<String>) -> Self {
        Self::Handshake(msg.into())
    }

    /// Creates a serialization error with the given message.
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }

    /// Creates an invalid payload error with the given message.
    pub fn invalid_payload(msg: impl Into<String>) -> Self {
        Self::InvalidPayload(msg.into())
    }

    /// Returns true if this is a transport error.
    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport(_))
    }

    /// Returns true if this is a timeout error.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }

    /// Returns true if this is a connection lost error.
    pub fn is_connection_lost(&self) -> bool {
        matches!(self, Self::ConnectionLost)
    }

    /// Returns true if this is a shutdown error.
    pub fn is_shutdown(&self) -> bool {
        matches!(self, Self::Shutdown)
    }
}

impl From<bincode::Error> for SyncError {
    fn from(err: bincode::Error) -> Self {
        SyncError::Serialization(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_error_display() {
        let err = SyncError::transport("connection refused");
        assert_eq!(err.to_string(), "Sync transport error: connection refused");
    }

    #[test]
    fn test_protocol_version_display() {
        let err = SyncError::ProtocolVersion {
            local: 1,
            remote: 2,
        };
        assert_eq!(
            err.to_string(),
            "Sync protocol version mismatch: local v1, remote v2"
        );
    }

    #[test]
    fn test_sync_error_is_checks() {
        assert!(SyncError::transport("x").is_transport());
        assert!(SyncError::Timeout.is_timeout());
        assert!(SyncError::ConnectionLost.is_connection_lost());
        assert!(SyncError::Shutdown.is_shutdown());
    }

    #[test]
    fn test_bincode_error_conversion() {
        // Deserializing truncated bytes triggers a bincode error
        let bad_bytes = vec![0u8; 1]; // too short for a (u64, u64)
        let bincode_err = bincode::deserialize::<(u64, u64)>(&bad_bytes).unwrap_err();
        let sync_err: SyncError = bincode_err.into();
        assert!(matches!(sync_err, SyncError::Serialization(_)));
    }
}
