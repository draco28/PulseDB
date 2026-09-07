//! Sync configuration types.
//!
//! [`SyncConfig`] controls the behavior of the sync protocol including
//! direction, conflict resolution, batch sizes, and retry policies.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;
use crate::storage::schema::MAX_CONTENT_SIZE;
use crate::types::CollectiveId;

/// Default for [`SyncConfig::max_request_bytes`]: 64 MiB.
///
/// Sized from the largest *valid* default batch, not from a typical one:
/// [`SyncConfig::batch_size`] defaults to 500 and a single experience's
/// `content` may be up to
/// [`MAX_CONTENT_SIZE`](crate::storage::schema::MAX_CONTENT_SIZE) = 100 KiB,
/// so a legitimate default batch can reach 500 x 100 KiB = 51 200 000 bytes
/// (~48.8 MiB) of content alone. There is no byte-aware batch splitting and no
/// shrink-and-retry, so a cap below that refuses a valid input and every later
/// cycle retries the same oversized body forever. 64 MiB (67 108 864 bytes)
/// clears that with room for the embeddings and metadata riding alongside the
/// content, while still refusing a runaway or hostile body long before it can
/// exhaust memory. It is deliberately **not** a bound on the theoretical
/// worst case — one experience may also carry up to `MAX_SOURCE_FILES` paths
/// and a large `applications` G-counter — because nothing here splits a batch
/// by bytes; byte-aware batching is protocol v5. `validate()` enforces the
/// `batch_size` relationship for any consumer that changes either value.
/// Locked by the r1 grill (Q4).
pub const DEFAULT_MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

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
    /// Default: 64 MiB ([`DEFAULT_MAX_REQUEST_BYTES`]). Must be greater than 0,
    /// and at least `batch_size * MAX_CONTENT_SIZE` — the largest batch the
    /// configured `batch_size` can legitimately produce — or `validate()`
    /// refuses the pair.
    ///
    /// Absent from a deserialized config in a **self-describing** format
    /// (JSON/TOML/YAML — a persisted 0.7.x one), it falls back to the default
    /// rather than failing the load. A **postcard**-encoded 0.7.x config does
    /// NOT load: postcard writes a struct as a fixed-length sequence carrying
    /// no field names, so the buffer ends before this field and the
    /// deserializer hits end-of-input — there is no missing *field* for a
    /// serde default to fill, only absent *bytes*. Re-encode such a config from
    /// a self-describing form, or from `SyncConfig::default()`.
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,

    /// Clock-skew allowance, in milliseconds, for an incoming experience's
    /// `last_reinforced` (#13).
    ///
    /// When a pulled or pushed reinforcement carries
    /// `last_reinforced > now + max_clock_skew_ms`, the applier counts it in
    /// [`SyncStats::skewed_timestamps`](super::types::SyncStats::skewed_timestamps)
    /// and logs the batch once at `warn` (peer, count, largest skew observed) —
    /// once per batch, not once per change: the condition never self-clears
    /// while the peer's clock is wrong.
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
    ///
    /// Absent from a deserialized config in a **self-describing** format
    /// (JSON/TOML/YAML — a persisted 0.7.x one), it falls back to the default
    /// rather than failing the load. A **postcard**-encoded 0.7.x config does
    /// NOT load: postcard writes a struct as a fixed-length sequence carrying
    /// no field names, so the buffer ends before this field and the
    /// deserializer hits end-of-input — there is no missing *field* for a
    /// serde default to fill, only absent *bytes*. Re-encode such a config from
    /// a self-describing form, or from `SyncConfig::default()`.
    #[serde(default = "default_max_clock_skew_ms")]
    pub max_clock_skew_ms: u64,
}

/// `serde` fallback for [`SyncConfig::max_request_bytes`].
///
/// The field was added in 0.8.0. Without this a persisted 0.7.x config in a
/// **self-describing** format (JSON, TOML, YAML) fails to load with
/// ``missing field `max_request_bytes` `` — a hard startup failure, not a
/// fallback.
///
/// **It rescues self-describing formats only.** A **postcard**-encoded 0.7.x
/// `SyncConfig` still fails to load, and no `#[serde(default)]` can change
/// that: postcard writes a struct as a fixed-length sequence carrying no field
/// names, so a 0.7.x buffer simply ends after `sync_insights` and the
/// deserializer hits end-of-input — there is no missing *field* for a default
/// to fill, only absent *bytes*. postcard is a supported representation of this
/// type (see `test_sync_config_postcard_roundtrip`), so a consumer that
/// persisted a `SyncConfig` that way must re-encode it from a self-describing
/// form, or from `SyncConfig::default()`, after upgrading.
/// A versioned or custom deserializer for that case is deferred to a tracked
/// issue.
fn default_max_request_bytes() -> usize {
    DEFAULT_MAX_REQUEST_BYTES
}

/// `serde` fallback for [`SyncConfig::max_clock_skew_ms`]; see
/// [`default_max_request_bytes`].
fn default_max_clock_skew_ms() -> u64 {
    DEFAULT_MAX_CLOCK_SKEW_MS
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
    /// - `max_request_bytes` is below `batch_size * MAX_CONTENT_SIZE`, the
    ///   largest batch `batch_size` can legitimately produce
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
        // A batch of `batch_size` experiences each carrying the maximum
        // `content` is a VALID input. Nothing splits a batch by bytes and
        // nothing shrinks and retries, so a cap below that size refuses a
        // legitimate push and every later cycle retries the same body forever.
        // A consumer who lowers one of the two must lower the other.
        let min_request_bytes = self.batch_size.saturating_mul(MAX_CONTENT_SIZE);
        if self.max_request_bytes < min_request_bytes {
            return Err(ValidationError::invalid_field(
                "max_request_bytes",
                format!(
                    "must be at least batch_size ({}) * MAX_CONTENT_SIZE ({} bytes) \
                     = {} bytes, the largest batch this batch_size can produce; \
                     lower batch_size or raise max_request_bytes",
                    self.batch_size, MAX_CONTENT_SIZE, min_request_bytes
                ),
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
        assert_eq!(config.max_request_bytes, 64 * 1024 * 1024);
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
    fn test_sync_config_default_admits_the_largest_valid_default_batch() {
        // 500 experiences x 100 KiB of content each is a VALID default batch.
        let config = SyncConfig::default();
        assert!(
            config.max_request_bytes >= config.batch_size * MAX_CONTENT_SIZE,
            "the default cap must not refuse a valid default batch"
        );
    }

    #[test]
    fn test_sync_config_validate_rejects_cap_below_a_valid_batch() {
        // Nothing splits a batch by bytes and nothing shrinks and retries, so a
        // cap below `batch_size * MAX_CONTENT_SIZE` would refuse a legitimate
        // push forever. The pair must be rejected at construction instead.
        let config = SyncConfig {
            batch_size: 500,
            max_request_bytes: 16 * 1024 * 1024,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        let ValidationError::InvalidField { field, reason } = err else {
            panic!("expected InvalidField");
        };
        assert_eq!(field, "max_request_bytes");
        assert!(reason.contains("batch_size"), "{reason}");
        assert!(reason.contains("51200000"), "{reason}");

        // Lowering the other side of the pair makes it valid again.
        let config = SyncConfig {
            batch_size: 100,
            max_request_bytes: 16 * 1024 * 1024,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_sync_config_validate_batch_size_multiply_saturates() {
        // A colossal batch_size must be refused, not wrap around to a tiny
        // minimum that lets an unbounded body through.
        let config = SyncConfig {
            batch_size: usize::MAX,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "max_request_bytes")
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

    /// A persisted 0.7.x `SyncConfig` carries neither `max_request_bytes` nor
    /// `max_clock_skew_ms`. Both were added in 0.8.0, so without
    /// `#[serde(default)]` a config stored in a **self-describing** format
    /// fails to load with ``missing field `max_request_bytes` `` — a hard
    /// startup failure rather than a fallback. (The defaults cannot rescue a
    /// postcard-encoded 0.7.x config; see [`default_max_request_bytes`].)
    #[test]
    fn test_sync_config_deserializes_a_0_7_x_payload_without_the_new_fields() {
        let legacy = r#"{
            "direction": "Bidirectional",
            "conflict_resolution": "ServerWins",
            "batch_size": 500,
            "push_interval_ms": 1000,
            "pull_interval_ms": 1000,
            "retry": {
                "max_retries": 5,
                "initial_backoff_ms": 500,
                "max_backoff_ms": 30000,
                "backoff_multiplier": 2.0
            },
            "collectives": null,
            "sync_relations": true,
            "sync_insights": true
        }"#;

        let config: SyncConfig =
            serde_json::from_str(legacy).expect("a 0.7.x config must still load");

        assert_eq!(config.max_request_bytes, DEFAULT_MAX_REQUEST_BYTES);
        assert_eq!(config.max_clock_skew_ms, DEFAULT_MAX_CLOCK_SKEW_MS);
        // The rest of the payload is preserved, and the result is usable.
        assert_eq!(config.batch_size, 500);
        assert_eq!(config.direction, SyncDirection::Bidirectional);
        assert!(config.validate().is_ok());
    }

    /// An explicit value in the payload still wins over the fallback.
    #[test]
    fn test_sync_config_deserialize_honours_explicit_new_fields() {
        let payload = r#"{
            "direction": "PushOnly",
            "conflict_resolution": "LastWriteWins",
            "batch_size": 10,
            "push_interval_ms": 1000,
            "pull_interval_ms": 1000,
            "retry": {
                "max_retries": 5,
                "initial_backoff_ms": 500,
                "max_backoff_ms": 30000,
                "backoff_multiplier": 2.0
            },
            "collectives": null,
            "sync_relations": true,
            "sync_insights": true,
            "max_request_bytes": 2097152,
            "max_clock_skew_ms": 42
        }"#;

        let config: SyncConfig = serde_json::from_str(payload).unwrap();
        assert_eq!(config.max_request_bytes, 2 * 1024 * 1024);
        assert_eq!(config.max_clock_skew_ms, 42);
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
