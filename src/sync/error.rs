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

    /// Wire-format preamble mismatch — caught by raw-byte inspection of the
    /// 3-byte preamble *before* any deserialize, so a serializer mismatch
    /// (e.g. a bincode-era peer vs a postcard-era peer) fails loud with a
    /// typed error instead of yielding garbage through the decoder.
    ///
    /// This is distinct from [`SyncError::ProtocolVersion`]: that variant is
    /// protocol-*semantics* (negotiated in-band after a successful decode);
    /// this variant is wire-*format* (the bytes can't be trusted to decode at
    /// all). Two failure shapes feed it:
    ///
    /// - **bad magic** (`got == None`): the leading bytes are not a PulseDB
    ///   sync preamble at all (truncated body, a non-PulseDB POST, or a
    ///   pre-4.0 no-preamble peer's body) — reported with `got: None`.
    /// - **wrong version** (`got == Some(v)`): a valid magic but a
    ///   `wire_format_version` byte this peer does not speak.
    #[error("Sync wire-format mismatch: expected wire format v{expected}, got {}", match got { Some(g) => format!("v{g}"), None => "bad/absent magic".to_string() })]
    WireFormatMismatch {
        /// The wire-format version this peer speaks.
        expected: u8,
        /// The wire-format version observed in the preamble, or `None` when the
        /// magic bytes were absent/wrong (so no trustworthy version was read).
        got: Option<u8>,
    },

    /// A request or response body exceeded the configured byte cap and was
    /// refused **before** any decode.
    ///
    /// Raised by the server-side byte handlers (`SyncServer::handle_*_bytes`)
    /// when `body.len()` exceeds
    /// [`SyncConfig::max_request_bytes`](super::config::SyncConfig::max_request_bytes),
    /// and by the HTTP transport client when a response body exceeds its cap.
    /// `size` is the observed body length — for a bounded read without a
    /// `Content-Length`, the byte count at which the cap was crossed — and
    /// `max` is the cap in force. The body never reaches postcard, so this is
    /// distinct from a decode-side [`SyncError::Serialization`].
    #[error("Sync payload too large: {size} bytes exceeds the {max}-byte cap")]
    PayloadTooLarge {
        /// Observed body length in bytes (or the length at which a bounded
        /// read crossed the cap).
        size: usize,
        /// The byte cap in force.
        max: usize,
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

    /// Creates a wire-format mismatch for a **bad / absent magic** preamble.
    ///
    /// Used when the leading bytes are not a recognizable PulseDB sync
    /// preamble (truncated body, wrong magic, or a pre-preamble peer's body).
    pub fn wire_format_bad_magic(expected: u8) -> Self {
        Self::WireFormatMismatch {
            expected,
            got: None,
        }
    }

    /// Creates a wire-format mismatch for a **wrong wire-format version**.
    ///
    /// Used when the magic is valid but the `wire_format_version` byte names a
    /// version this peer does not speak.
    pub fn wire_format_version(expected: u8, got: u8) -> Self {
        Self::WireFormatMismatch {
            expected,
            got: Some(got),
        }
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

    /// Returns true if this is a wire-format mismatch (bad magic OR wrong
    /// wire-format version) — the typed fail-loud signal for cross-version
    /// sync, distinct from a generic [`SyncError::Serialization`].
    pub fn is_wire_format_mismatch(&self) -> bool {
        matches!(self, Self::WireFormatMismatch { .. })
    }

    /// Returns true if a body was refused for exceeding the byte cap before
    /// decode — the signal a consumer maps to `413 Payload Too Large`.
    pub fn is_payload_too_large(&self) -> bool {
        matches!(self, Self::PayloadTooLarge { .. })
    }
}

impl From<postcard::Error> for SyncError {
    fn from(err: postcard::Error) -> Self {
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
    fn test_postcard_error_conversion() {
        // Deserializing truncated bytes triggers a postcard error.
        let bad_bytes = vec![0u8; 0]; // too short for a (u64, u64)
        let postcard_err = postcard::from_bytes::<(u64, u64)>(&bad_bytes).unwrap_err();
        let sync_err: SyncError = postcard_err.into();
        assert!(matches!(sync_err, SyncError::Serialization(_)));
    }

    #[test]
    fn test_payload_too_large_typed_and_distinct() {
        let err = SyncError::PayloadTooLarge { size: 65, max: 64 };
        assert!(err.is_payload_too_large());
        assert!(!err.is_wire_format_mismatch());
        assert!(!SyncError::serialization("x").is_payload_too_large());
        assert_eq!(
            err.to_string(),
            "Sync payload too large: 65 bytes exceeds the 64-byte cap"
        );
    }

    #[test]
    fn test_wire_format_mismatch_typed_and_distinct() {
        let bad_magic = SyncError::wire_format_bad_magic(3);
        let wrong_ver = SyncError::wire_format_version(3, 2);

        // Both are the typed wire-format signal, NOT a generic Serialization.
        assert!(bad_magic.is_wire_format_mismatch());
        assert!(wrong_ver.is_wire_format_mismatch());
        assert!(!SyncError::serialization("x").is_wire_format_mismatch());

        // Distinct from protocol-semantics ProtocolVersion.
        assert!(matches!(
            bad_magic,
            SyncError::WireFormatMismatch { got: None, .. }
        ));
        assert!(matches!(
            wrong_ver,
            SyncError::WireFormatMismatch {
                expected: 3,
                got: Some(2)
            }
        ));
    }
}
