//! Sync configuration types.
//!
//! [`SyncConfig`] controls the behavior of the sync protocol including
//! direction, conflict resolution, batch sizes, and retry policies.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;
use crate::types::CollectiveId;

/// Default for [`SyncConfig::max_request_bytes`]: 16 MiB.
///
/// Roughly 8x a default 500-experience batch (384-dim embeddings included),
/// so a healthy peer never trips it while a runaway or hostile body is refused
/// long before it can exhaust memory. Locked by the r1 grill (Q4).
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;

/// Default for [`SyncConfig::max_clock_skew_ms`]: 300 000 ms (5 minutes).
///
/// Wide enough that ordinary NTP drift between peers never trips it, tight
/// enough that a runaway reinforcement timestamp is surfaced. Locked by the r1
/// grill (Q4).
pub const DEFAULT_MAX_CLOCK_SKEW_MS: u64 = 300_000;

// ============================================================================
// SyncDirection
// ============================================================================

/// Direction of sync data flow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncDirection {
    /// Only push local changes to the remote peer.
    PushOnly,
    /// Only pull remote changes to the local instance.
    PullOnly,
    /// Both push and pull (full bidirectional sync).
    #[default]
    Bidirectional,
}

// ============================================================================
// ConflictResolution
// ============================================================================

/// Strategy for resolving conflicts when the same entity is modified
/// on both peers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// Remote (server) changes always win on conflict.
    #[default]
    ServerWins,
    /// The change with the latest timestamp wins.
    LastWriteWins,
}

// ============================================================================
// RetryConfig
// ============================================================================

/// Configuration for retry behavior on transient sync failures.
///
/// Uses exponential backoff with a configurable multiplier and cap.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of consecutive retries before giving up.
    pub max_retries: u32,

    /// Initial backoff duration in milliseconds.
    pub initial_backoff_ms: u64,

    /// Maximum backoff duration in milliseconds (cap).
    pub max_backoff_ms: u64,

    /// Multiplier applied to backoff after each retry.
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 5,
            initial_backoff_ms: 500,
            max_backoff_ms: 30_000,
            backoff_multiplier: 2.0,
        }
    }
}

// ============================================================================
// SyncConfig
// ============================================================================

/// Configuration for the sync protocol.
///
/// Controls direction, conflict resolution, batch sizes, polling intervals,
/// and which collectives to sync.
///
/// # Example
/// ```
/// use pulsedb::sync::config::{SyncConfig, SyncDirection};
///
/// let config = SyncConfig {
///     direction: SyncDirection::PushOnly,
///     batch_size: 200,
///     ..Default::default()
/// };
/// assert!(config.validate().is_ok());
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Direction of sync data flow.
    pub direction: SyncDirection,

    /// Strategy for resolving conflicts.
    pub conflict_resolution: ConflictResolution,

    /// Maximum number of changes per sync batch.
    ///
    /// Larger batches reduce round trips but increase memory usage.
    /// Default: 500
    pub batch_size: usize,

    /// Interval between push cycles in milliseconds.
    ///
    /// Default: 1000 (1 second)
    pub push_interval_ms: u64,

    /// Interval between pull cycles in milliseconds.
    ///
    /// Default: 1000 (1 second)
    pub pull_interval_ms: u64,

    /// Retry configuration for transient failures.
    pub retry: RetryConfig,

    /// Optional filter: only sync these collectives.
    ///
    /// `None` means sync all collectives.
    pub collectives: Option<Vec<CollectiveId>>,

    /// Whether to sync experience relations.
    ///
    /// Default: true
    pub sync_relations: bool,

    /// Whether to sync derived insights.
    ///
    /// Default: true
    pub sync_insights: bool,

    /// Upper bound, in bytes, on a single sync request body accepted by the
    /// server-side byte handlers (`SyncServer::handle_*_bytes`, `sync-http`).
    ///
    /// The check is `bytes.len() <= max_request_bytes`, made **before** the
    /// wire preamble is read and before any postcard decode, so an oversized
    /// body is refused with the typed
    /// [`SyncError::PayloadTooLarge`](super::error::SyncError::PayloadTooLarge)
    /// and never reaches the decoder. PulseDB builds no router of its own, so
    /// this is the only cap it enforces at the network edge (ADR-009); a
    /// consumer's framework body limit applies in addition, not instead.
    ///
    /// The HTTP transport client applies the same default to *response* bodies
    /// (see `HttpSyncTransport::with_max_response_bytes`).
    ///
    /// Default: 16 MiB ([`DEFAULT_MAX_REQUEST_BYTES`]). Must be greater than 0.
    pub max_request_bytes: usize,

    /// Clock-skew allowance, in milliseconds, for an incoming experience's
    /// `last_reinforced` (#13).
    ///
    /// When a pulled or pushed reinforcement carries
    /// `last_reinforced > now + max_clock_skew_ms`, the applier logs it at
    /// `warn` (peer, experience id, skew) and counts it in
    /// [`SyncStats::skewed_timestamps`](super::types::SyncStats::skewed_timestamps).
    /// The value is still merged exactly as FR-031's max-merge dictates — it is
    /// **never** clamped, rejected or re-timestamped — so two peers keep
    /// converging on the same bytes.
    ///
    /// **The bound is advisory until protocol v5.** A bound that also corrects
    /// the value needs a record-carried time reference to converge on, and
    /// that is a wire change scheduled for protocol v5 (Release 2). Until then
    /// this setting only decides what is *surfaced*, never what is *stored*.
    ///
    /// Default: 300 000 (5 minutes, [`DEFAULT_MAX_CLOCK_SKEW_MS`]).
    pub max_clock_skew_ms: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            direction: SyncDirection::default(),
            conflict_resolution: ConflictResolution::default(),
            batch_size: 500,
            push_interval_ms: 1000,
            pull_interval_ms: 1000,
            retry: RetryConfig::default(),
            collectives: None,
            sync_relations: true,
            sync_insights: true,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            max_clock_skew_ms: DEFAULT_MAX_CLOCK_SKEW_MS,
        }
    }
}

impl SyncConfig {
    /// Validates the sync configuration.
    ///
    /// # Errors
    /// Returns `ValidationError` if:
    /// - `batch_size` is 0
    /// - `push_interval_ms` is 0
    /// - `pull_interval_ms` is 0
    /// - `max_request_bytes` is 0
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.batch_size == 0 {
            return Err(ValidationError::invalid_field(
                "batch_size",
                "must be greater than 0",
            ));
        }
        if self.push_interval_ms == 0 {
            return Err(ValidationError::invalid_field(
                "push_interval_ms",
                "must be greater than 0",
            ));
        }
        if self.pull_interval_ms == 0 {
            return Err(ValidationError::invalid_field(
                "pull_interval_ms",
                "must be greater than 0",
            ));
        }
        if self.max_request_bytes == 0 {
            return Err(ValidationError::invalid_field(
                "max_request_bytes",
                "must be greater than 0",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_config_defaults() {
        let config = SyncConfig::default();
        assert_eq!(config.direction, SyncDirection::Bidirectional);
        assert_eq!(config.conflict_resolution, ConflictResolution::ServerWins);
        assert_eq!(config.batch_size, 500);
        assert_eq!(config.push_interval_ms, 1000);
        assert_eq!(config.pull_interval_ms, 1000);
        assert!(config.collectives.is_none());
        assert!(config.sync_relations);
        assert!(config.sync_insights);
        assert_eq!(config.max_request_bytes, 16 * 1024 * 1024);
        assert_eq!(config.max_request_bytes, DEFAULT_MAX_REQUEST_BYTES);
        assert_eq!(config.max_clock_skew_ms, 300_000);
        assert_eq!(config.max_clock_skew_ms, DEFAULT_MAX_CLOCK_SKEW_MS);
    }

    #[test]
    fn test_sync_config_validate_success() {
        let config = SyncConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sync_config_validate_zero_max_request_bytes() {
        let config = SyncConfig {
            max_request_bytes: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "max_request_bytes")
        );
    }

    #[test]
    fn test_sync_config_validate_zero_batch_size() {
        let config = SyncConfig {
            batch_size: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "batch_size")
        );
    }

    #[test]
    fn test_sync_config_validate_zero_push_interval() {
        let config = SyncConfig {
            push_interval_ms: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "push_interval_ms")
        );
    }

    #[test]
    fn test_sync_config_validate_zero_pull_interval() {
        let config = SyncConfig {
            pull_interval_ms: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "pull_interval_ms")
        );
    }

    #[test]
    fn test_sync_config_postcard_roundtrip() {
        let config = SyncConfig {
            direction: SyncDirection::PushOnly,
            batch_size: 100,
            collectives: Some(vec![CollectiveId::new()]),
            ..Default::default()
        };
        let bytes = postcard::to_allocvec(&config).unwrap();
        let restored: SyncConfig = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(config.direction, restored.direction);
        assert_eq!(config.batch_size, restored.batch_size);
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_backoff_ms, 500);
        assert_eq!(config.max_backoff_ms, 30_000);
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }
}
