//! Sync configuration types.
//!
//! [`SyncConfig`] controls the behavior of the sync protocol including
//! direction, conflict resolution, batch sizes, and retry policies.

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;
use crate::storage::schema::{
    MAX_CONTENT_SIZE, MAX_DOMAIN_TAGS, MAX_FILE_PATH_LENGTH, MAX_KV_TAGS, MAX_KV_TAG_KEY_LENGTH,
    MAX_KV_TAG_VALUE_LENGTH, MAX_SOURCE_AGENT_LENGTH, MAX_SOURCE_FILES, MAX_TAG_LENGTH,
};
use crate::types::CollectiveId;

/// Postcard framing allowance, in bytes, carried per synced experience: 1 KiB.
///
/// A sum of field-length limits is **not** on its own an upper bound on encoded
/// size. postcard writes a length varint ahead of every string and every
/// collection, varint-encodes integers, and the record also carries fixed-width
/// fields no schema constant covers — `id`, `collective_id`, `importance`,
/// `confidence`, `timestamp`, `last_reinforced`, `archived`, the
/// `experience_type` discriminant — plus the
/// [`SyncChange`](super::types::SyncChange) envelope it travels in. Without an
/// allowance for all that, a bare field-length sum sits *under* the real
/// encoding.
///
/// This term is the one that was **measured** rather than derived: an
/// experience at every bounded limit encodes 532 bytes past the sum of its
/// field limits (`test_a_maximum_field_default_batch_fits_the_default_cap`).
/// 1 KiB is that rounded up to roughly double, so
/// [`MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`] stays above the encoder
/// even if a fixed-width field is added or a length prefix widens.
const EXPERIENCE_WIRE_ENVELOPE_BYTES: usize = 1024;

/// Upper bound, in bytes, on one synced experience's encoded size across every
/// bounded field **except `applications`**: 173 680 (~169.6 KiB).
///
/// Derived from the [schema](crate::storage::schema) constants rather than
/// written down, so it cannot drift when a bound moves:
///
/// | Term | From | Bytes |
/// |---|---|---|
/// | `content` | `MAX_CONTENT_SIZE` | 102 400 |
/// | `related_files` | `MAX_SOURCE_FILES` (100) x `MAX_FILE_PATH_LENGTH` (500) | 50 000 |
/// | `tags` | `MAX_KV_TAGS` (50) x (`MAX_KV_TAG_KEY_LENGTH` 100 + `MAX_KV_TAG_VALUE_LENGTH` 200) | 15 000 |
/// | `domain` | `MAX_DOMAIN_TAGS` (50) x `MAX_TAG_LENGTH` (100) | 5 000 |
/// | `source_agent` | `MAX_SOURCE_AGENT_LENGTH` | 256 |
/// | postcard framing + fixed-width fields | `EXPERIENCE_WIRE_ENVELOPE_BYTES` | 1 024 |
/// | **total** | | **173 680** |
///
/// [`SyncConfig::validate`] floors `max_request_bytes` at
/// `batch_size * MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`, replacing an
/// earlier floor that counted `content` alone — content is only 59% of what a
/// valid experience may carry, and at the pre-0.8.0 default `batch_size` of 500
/// the other bounded fields alone took the batch to ~86.8 MB against a 64 MiB
/// cap.
///
/// # These are field-size limits, not measured wire sizes
///
/// The first five terms are each field's declared maximum *payload*, used as a
/// conservative proxy for its encoded size. They are not measurements, and a
/// bare sum of them would understate the encoding; the framing term is what
/// closes that gap, and it is the one term measured against the real encoder.
///
/// The bound is checked end to end rather than argued.
/// `test_a_maximum_field_default_batch_fits_the_default_cap` builds a default
/// batch of experiences populated to every bounded limit, encodes it exactly as
/// the push path does (`postcard::to_allocvec` over `Vec<SyncChange>`), and
/// asserts the body fits [`DEFAULT_MAX_REQUEST_BYTES`]. It measures
/// **43 297 002 bytes** — against this bound's 43 420 000 for the same batch and
/// a cap of 67 108 864, so the floor sits above the encoder and below the cap.
///
/// # Residual: `applications` is excluded (issue #98)
///
/// `Experience::applications` is a G-counter — one bucket per replica that
/// reinforced the experience — and `RemoteChangeApplier` accepts up to
/// `MAX_SYNC_APPLICATION_BUCKETS` (65 536) of them on a single incoming
/// experience. Each bucket costs about 22 bytes encoded (a 16-byte `InstanceId`
/// with its length prefix, plus a varint `u32`), so **an experience carrying
/// enough buckets exceeds this bound on its own, and a batch of them still
/// exceeds `max_request_bytes` even though the pair passed `validate()`.**
///
/// It does not take the applier's maximum to get there. At the default
/// `batch_size` the measured batch above leaves 23 811 862 bytes of headroom
/// under the default cap — about 95 KB per experience, or roughly **4 300**
/// distinct peer instances each. That is the real threshold, not 65 536.
///
/// It is excluded anyway because folding it in puts the per-experience bound at
/// ~1.54 MiB, which a 64 MiB cap covers for a `batch_size` of only 41 — a more
/// than tenfold throughput cut to defend a case that needs thousands of
/// distinct instances reinforcing one experience.
///
/// When a batch does overrun the cap, the peer refuses the request with
/// [`SyncError::PayloadTooLarge`](super::error::SyncError::PayloadTooLarge).
/// There is no byte-aware batch splitting and no shrink-and-retry, so the next
/// cycle rebuilds the identical batch and is refused again, and sync stops
/// making progress until an operator lowers `batch_size` or raises
/// `max_request_bytes`. **Issue #98** is where that is fixed.
///
/// # Residual: `embedding` (issue #96)
///
/// `Experience::embedding` is `#[serde(skip)]` today, so it costs nothing on the
/// wire and is excluded here deliberately rather than overlooked. Once #96's
/// wire half lands it adds `dimensions * 4` bytes plus a length varint per
/// experience — 6 KiB at 1536 dimensions — and this arithmetic must be revisited
/// with it.
pub const MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS: usize = MAX_CONTENT_SIZE
    + MAX_SOURCE_FILES * MAX_FILE_PATH_LENGTH
    + MAX_KV_TAGS * (MAX_KV_TAG_KEY_LENGTH + MAX_KV_TAG_VALUE_LENGTH)
    + MAX_DOMAIN_TAGS * MAX_TAG_LENGTH
    + MAX_SOURCE_AGENT_LENGTH
    + EXPERIENCE_WIRE_ENVELOPE_BYTES;

/// Default for [`SyncConfig::max_request_bytes`]: 64 MiB.
///
/// Sized against the largest batch the default `batch_size` can legitimately
/// build out of bounded fields: 250 x
/// [`MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`] = 43 420 000 bytes
/// (~41.4 MiB), which a real maximum-field batch comes in under at a measured
/// 43 297 002 bytes. 64 MiB (67 108 864 bytes) clears that with ~22.7 MiB to
/// spare, while still refusing a runaway or hostile body long before it can
/// exhaust memory. At this cap `validate()` admits a `batch_size` up to **386**.
///
/// **It is not a bound on every valid batch.** `applications` is outside the
/// per-experience bound (see that constant), so an experience carrying roughly
/// 4 300 or more G-counter buckets can take a default batch past this cap even
/// though the configuration validates. There is then no byte-aware splitting and
/// no shrink-and-retry: the request is refused with
/// [`SyncError::PayloadTooLarge`](super::error::SyncError::PayloadTooLarge), the
/// next cycle rebuilds the identical batch and is refused again, and sync stops
/// making progress. **Issue #98** is where that is fixed; raising this cap is
/// not.
///
/// A consumer who raises `batch_size` must raise this cap with it — `validate()`
/// enforces the relationship. Locked by the r1 grill (Q4).
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
    /// Default: 250
    ///
    /// The default was 500 before 0.8.0. It came down because
    /// [`validate`](Self::validate) now floors
    /// [`max_request_bytes`](Self::max_request_bytes) at `batch_size` x
    /// [`MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`] rather than at
    /// content alone, and 500 of those is ~86.8 MB against a 64 MiB cap — a
    /// batch the default configuration could build and no peer would accept.
    /// At the default cap the largest `batch_size` that validates is **386**.
    ///
    /// **A batch that validates can still overrun the cap.**
    /// `applications` is outside that per-experience bound: an experience
    /// carrying roughly 4 300 or more G-counter buckets takes a default batch
    /// past 64 MiB even though the pair passed `validate()`. When it does, the
    /// peer refuses the request with
    /// [`SyncError::PayloadTooLarge`](super::error::SyncError::PayloadTooLarge),
    /// and with no byte-aware splitting and no shrink-and-retry every following
    /// cycle rebuilds the identical batch and is refused again — sync stops
    /// making progress until an operator lowers `batch_size` or raises the cap.
    /// **Issue #98** is where that is fixed. `Experience::embedding` is
    /// `#[serde(skip)]` today and will add `dimensions * 4` bytes per experience
    /// once **issue #96**'s wire half lands, which moves these numbers again.
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
    /// and at least `batch_size` x
    /// [`MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`] — the largest batch
    /// of bounded fields the configured `batch_size` can build — or `validate()`
    /// refuses the pair. Before 0.8.0 that floor counted `content` alone, which
    /// let the shipped defaults build an ~86.8 MB batch against this 64 MiB cap.
    ///
    /// **The floor is not a guarantee that every valid batch fits.**
    /// `applications` is deliberately outside the per-experience bound, so an
    /// experience carrying roughly 4 300 or more G-counter buckets takes a
    /// default-size batch past this cap even though the configuration validated.
    /// There is no byte-aware batch splitting and no shrink-and-retry: the
    /// request is refused with
    /// [`SyncError::PayloadTooLarge`](super::error::SyncError::PayloadTooLarge),
    /// the next cycle rebuilds the identical batch and is refused again, and
    /// sync stops making progress until `batch_size` comes down or this cap goes
    /// up. **Issue #98** is where that is fixed. `Experience::embedding` adds
    /// `dimensions * 4` bytes per experience once **issue #96**'s wire half
    /// lands, which this arithmetic will have to absorb.
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
            batch_size: 250,
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
    /// - `max_request_bytes` is below `batch_size` x
    ///   [`MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`], the largest batch
    ///   of bounded fields `batch_size` can build
    ///
    /// # What this does not check
    ///
    /// That last floor covers every bounded field **except `applications`**,
    /// which is unbounded for this purpose — the applier accepts up to
    /// `MAX_SYNC_APPLICATION_BUCKETS` (65 536) G-counter buckets per experience
    /// and roughly 4 300 of them is already enough to take a default-size batch
    /// past the default cap. A configuration that passes here can therefore
    /// still build a batch its peer refuses with
    /// [`SyncError::PayloadTooLarge`](super::error::SyncError::PayloadTooLarge),
    /// and with no byte-aware splitting and no shrink-and-retry every following
    /// cycle rebuilds the identical batch and is refused again. Folding
    /// `applications` into the floor would force a default `batch_size` near 41;
    /// the fix is byte-aware batching, tracked in **issue #98**.
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
        // A batch of `batch_size` experiences each at every bounded field's
        // maximum is a VALID input. Nothing splits a batch by bytes and nothing
        // shrinks and retries, so a cap below that size refuses a legitimate
        // push and every later cycle retries the same body forever. A consumer
        // who raises one of the two must raise the other.
        //
        // `applications` is outside the bound (see the constant): it is capped
        // only by the applier's MAX_SYNC_APPLICATION_BUCKETS, and folding that
        // in would force a default batch_size near 41. So this floor is
        // necessary, not sufficient — the residual is issue #98.
        let min_request_bytes = self
            .batch_size
            .saturating_mul(MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS);
        if self.max_request_bytes < min_request_bytes {
            return Err(ValidationError::invalid_field(
                "max_request_bytes",
                format!(
                    "must be at least batch_size ({}) * \
                     MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS ({} bytes) = {} bytes, \
                     the largest batch of bounded fields this batch_size can build; \
                     lower batch_size or raise max_request_bytes",
                    self.batch_size,
                    MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS,
                    min_request_bytes
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
        assert_eq!(config.batch_size, 250);
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

    /// The shipped defaults must satisfy their own floor — with headroom, and
    /// asserted through `validate()` rather than by re-deriving the arithmetic,
    /// so a future default change cannot silently ship a configuration that
    /// builds a batch no peer will accept.
    #[test]
    fn test_sync_config_default_admits_the_largest_valid_default_batch() {
        let config = SyncConfig::default();
        config
            .validate()
            .expect("the shipped defaults must pass their own validation");

        // 250 experiences at every bounded field's maximum: 43 420 000 bytes
        // against a 64 MiB cap.
        let worst_case = config.batch_size * MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS;
        assert_eq!(worst_case, 43_420_000);
        assert!(
            config.max_request_bytes >= worst_case,
            "the default cap must not refuse a valid default batch"
        );
        assert!(
            config.max_request_bytes - worst_case > 16 * 1024 * 1024,
            "the defaults are meant to clear the floor with real headroom, not sit on it"
        );
    }

    /// The ceiling is a boundary, so test it as one: the largest `batch_size`
    /// the default cap admits passes, and one more fails.
    #[test]
    fn test_sync_config_validate_batch_size_ceiling_is_exact() {
        let ceiling = DEFAULT_MAX_REQUEST_BYTES / MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS;
        assert_eq!(ceiling, 386, "the documented ceiling at the default cap");

        let at_ceiling = SyncConfig {
            batch_size: ceiling,
            ..Default::default()
        };
        at_ceiling
            .validate()
            .expect("the largest batch_size the default cap covers must be accepted");

        let over_ceiling = SyncConfig {
            batch_size: ceiling + 1,
            ..Default::default()
        };
        let err = over_ceiling.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { ref field, .. } if field == "max_request_bytes"),
            "one past the ceiling must be refused, got {err:?}"
        );
    }

    /// The pre-0.8.0 default `batch_size` against the unchanged default cap:
    /// 500 x 173 680 = 86 840 000 bytes over a 67 108 864-byte cap. This is the
    /// breaking half of the change, and the error has to name both sides of the
    /// pair so an operator can act on it.
    #[test]
    fn test_sync_config_validate_rejects_the_old_default_batch_size() {
        let config = SyncConfig {
            batch_size: 500,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        let ValidationError::InvalidField { field, reason } = err else {
            panic!("expected InvalidField");
        };
        assert_eq!(field, "max_request_bytes");
        assert!(reason.contains("batch_size (500)"), "{reason}");
        assert!(reason.contains("max_request_bytes"), "{reason}");
        assert!(reason.contains("86840000"), "{reason}");
    }

    #[test]
    fn test_sync_config_validate_rejects_cap_below_a_valid_batch() {
        // Nothing splits a batch by bytes and nothing shrinks and retries, so a
        // cap below `batch_size * MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`
        // would refuse a legitimate push forever. The pair must be rejected at
        // construction instead.
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
        assert!(reason.contains("86840000"), "{reason}");

        // A batch_size the content-only floor accepted at this cap (100 x 100 KiB
        // = 10 240 000 bytes) is refused now, because the batch it can really
        // build is 17 368 000 bytes.
        let config = SyncConfig {
            batch_size: 100,
            max_request_bytes: 16 * 1024 * 1024,
            ..Default::default()
        };
        assert!(
            config.validate().is_err(),
            "the content-only floor accepted this pair; the field-wide floor must not"
        );

        // Lowering the other side of the pair makes it valid again.
        let config = SyncConfig {
            batch_size: 90,
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
        // The rest of the payload is preserved.
        assert_eq!(config.batch_size, 500);
        assert_eq!(config.direction, SyncDirection::Bidirectional);

        // ...but 500 was the 0.7.x default `batch_size`, and it no longer
        // validates against the (unchanged) default cap: 500 x 173 680 is
        // 86 840 000 bytes over 67 108 864. That is the breaking half of this
        // change, and this is where a carried-forward config meets it. Loading
        // still succeeds; it is `validate()` that refuses.
        let err = config
            .validate()
            .expect_err("a carried-forward batch_size of 500 must not validate at the default cap");
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "max_request_bytes")
        );

        // Adopting the new default fixes it without touching the cap.
        let migrated = SyncConfig {
            batch_size: SyncConfig::default().batch_size,
            ..config
        };
        assert!(migrated.validate().is_ok());
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

    /// The claim this whole class rests on, measured rather than reasoned: a
    /// DEFAULT-sized batch of experiences at every bounded limit, encoded
    /// exactly as the push path encodes it, fits [`DEFAULT_MAX_REQUEST_BYTES`].
    ///
    /// A sum of field-length limits is not by itself an upper bound on encoded
    /// size — postcard frames every string and collection with a length varint
    /// and the record carries fixed-width fields besides — so
    /// [`MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS`] adds
    /// `EXPERIENCE_WIRE_ENVELOPE_BYTES` on top of the schema bounds. This test
    /// pins both ends: the encoder must come in under the derived bound, and the
    /// derived bound must come in under the cap.
    ///
    /// `applications` is left EMPTY on purpose. It is outside the bound (see the
    /// constant), and the residual it leaves — roughly 4 300 buckets per
    /// experience is enough to overrun the cap at this batch size — is what
    /// issue #98 fixes.
    #[test]
    fn test_a_maximum_field_default_batch_fits_the_default_cap() {
        use std::collections::BTreeMap;

        use crate::experience::{Experience, ExperienceType, Severity};
        use crate::sync::types::{SyncChange, SyncEntityType, SyncPayload};
        use crate::types::{AgentId, ExperienceId, InstanceId, Timestamp};

        let config = SyncConfig::default();
        let collective_id = CollectiveId::new();

        let max_field_change = || SyncChange {
            sequence: u64::MAX,
            source_instance: InstanceId::new(),
            collective_id,
            entity_type: SyncEntityType::Experience,
            payload: SyncPayload::ExperienceCreated(Experience {
                id: ExperienceId::new(),
                collective_id,
                content: "c".repeat(MAX_CONTENT_SIZE),
                // `#[serde(skip)]` today — zero wire bytes until issue #96.
                embedding: Vec::new(),
                experience_type: ExperienceType::Difficulty {
                    description: String::new(),
                    severity: Severity::Critical,
                },
                importance: 1.0,
                confidence: 1.0,
                // Outside the bound by design — the issue-#98 residual.
                applications: BTreeMap::new(),
                domain: (0..MAX_DOMAIN_TAGS)
                    .map(|_| "d".repeat(MAX_TAG_LENGTH))
                    .collect(),
                tags: (0..MAX_KV_TAGS)
                    .map(|i| {
                        (
                            format!("{i:0width$}", width = MAX_KV_TAG_KEY_LENGTH),
                            "v".repeat(MAX_KV_TAG_VALUE_LENGTH),
                        )
                    })
                    .collect(),
                related_files: (0..MAX_SOURCE_FILES)
                    .map(|_| "p".repeat(MAX_FILE_PATH_LENGTH))
                    .collect(),
                source_agent: AgentId::new("a".repeat(MAX_SOURCE_AGENT_LENGTH)),
                source_task: None,
                timestamp: Timestamp(i64::MAX),
                last_reinforced: Timestamp(i64::MAX),
                archived: false,
            }),
            timestamp: Timestamp(i64::MAX),
        };

        // `SyncServer::handle_push_bytes` decodes exactly this: a postcard
        // `Vec<SyncChange>`, with the byte cap applied to the encoded body.
        let batch: Vec<SyncChange> = (0..config.batch_size).map(|_| max_field_change()).collect();
        let body = postcard::to_allocvec(&batch).unwrap().len();

        let derived = config.batch_size * MAX_EXPERIENCE_WIRE_BYTES_EXCLUDING_APPLICATIONS;
        assert!(
            body <= derived,
            "a field-length sum is not an encoded bound on its own: the encoder \
             produced {body} bytes against a derived bound of {derived}. Raise \
             EXPERIENCE_WIRE_ENVELOPE_BYTES."
        );
        assert!(
            body <= config.max_request_bytes,
            "a maximum-field default batch encodes to {body} bytes, over the \
             default cap of {}",
            config.max_request_bytes
        );

        // The framing allowance must be doing real work — if the raw field sum
        // already covered the encoding, the allowance would be dead weight and
        // the comment claiming otherwise would be wrong.
        let field_sum = config.batch_size
            * (MAX_CONTENT_SIZE
                + MAX_SOURCE_FILES * MAX_FILE_PATH_LENGTH
                + MAX_KV_TAGS * (MAX_KV_TAG_KEY_LENGTH + MAX_KV_TAG_VALUE_LENGTH)
                + MAX_DOMAIN_TAGS * MAX_TAG_LENGTH
                + MAX_SOURCE_AGENT_LENGTH);
        assert!(
            body > field_sum,
            "postcard framing was expected to push the encoding ({body}) past the \
             raw field sum ({field_sum})"
        );
    }
}
