//! Sync configuration types.
//!
//! [`SyncConfig`] controls the behavior of the sync protocol including
//! direction, conflict resolution, batch sizes, and retry policies.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;
use crate::types::CollectiveId;

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
    }

    #[test]
    fn test_sync_config_validate_success() {
        let config = SyncConfig::default();
        assert!(config.validate().is_ok());
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
    fn test_sync_config_bincode_roundtrip() {
        let config = SyncConfig {
            direction: SyncDirection::PushOnly,
            batch_size: 100,
            collectives: Some(vec![CollectiveId::new()]),
            ..Default::default()
        };
        let bytes = bincode::serialize(&config).unwrap();
        let restored: SyncConfig = bincode::deserialize(&bytes).unwrap();
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
