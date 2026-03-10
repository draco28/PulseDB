//! Cross-process change detection via WAL sequence polling.
//!
//! Reader processes use [`ChangePoller`] to discover experience mutations
//! made by the writer process. Each poller maintains an independent cursor
//! (last seen sequence number), enabling multiple readers to poll at their
//! own pace without interfering with each other.
//!
//! # Architecture
//!
//! ```text
//! Writer Process                    Reader Process(es)
//! ┌──────────────┐                 ┌──────────────────┐
//! │ PulseDB      │                 │ ChangePoller     │
//! │ ├── write ───┤──► redb ◄──────┤── poll_changes() │
//! │ │ (seq++)    │   (shared      │ (reads WAL log)  │
//! │ └────────────┘    file)       └──────────────────┘
//! └──────────────┘
//! ```

use crate::error::Result;
use crate::storage::StorageEngine;
use crate::watch::WatchEvent;

/// Default maximum number of events returned per poll call.
const DEFAULT_BATCH_LIMIT: usize = 1000;

/// Cross-process change poller.
///
/// Tracks a cursor position in the WAL event log and returns new events
/// on each `poll()` call. Each poller is independent — multiple readers
/// can poll the same database at different rates.
///
/// # Example
///
/// ```rust,ignore
/// use pulsedb::ChangePoller;
///
/// let mut poller = ChangePoller::new();
///
/// loop {
///     let events = poller.poll(&storage)?;
///     for event in events {
///         println!("Change: {:?}", event.event_type);
///     }
///     std::thread::sleep(Duration::from_millis(100));
/// }
/// ```
pub struct ChangePoller {
    /// Last sequence number successfully consumed.
    last_seq: u64,

    /// Maximum events returned per poll call.
    batch_limit: usize,
}

impl ChangePoller {
    /// Creates a new poller starting from sequence 0 (receives all events).
    pub fn new() -> Self {
        Self {
            last_seq: 0,
            batch_limit: DEFAULT_BATCH_LIMIT,
        }
    }

    /// Creates a poller starting from a specific sequence number.
    ///
    /// Events with sequence <= `seq` will not be returned. Use this to
    /// resume polling from a previously saved position.
    pub fn from_sequence(seq: u64) -> Self {
        Self {
            last_seq: seq,
            batch_limit: DEFAULT_BATCH_LIMIT,
        }
    }

    /// Creates a poller with a custom batch limit.
    pub fn with_batch_limit(mut self, limit: usize) -> Self {
        self.batch_limit = limit;
        self
    }

    /// Returns the last sequence number this poller has consumed.
    ///
    /// Save this value to resume polling after a restart.
    pub fn last_sequence(&self) -> u64 {
        self.last_seq
    }

    /// Polls for new experience changes since the last call.
    ///
    /// Returns new [`WatchEvent`]s in sequence order and advances the
    /// internal cursor. Returns an empty vec if no new changes exist.
    ///
    /// # Performance
    ///
    /// Target: < 10ms per call. This performs a range scan on the
    /// watch_events redb table, which is O(k) where k = number of
    /// new events (not total events).
    pub fn poll(&mut self, storage: &dyn StorageEngine) -> Result<Vec<WatchEvent>> {
        let (records, new_seq) = storage.poll_watch_events(self.last_seq, self.batch_limit)?;
        self.last_seq = new_seq;
        Ok(records.into_iter().map(WatchEvent::from).collect())
    }
}

impl Default for ChangePoller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_poller_new_starts_at_zero() {
        let poller = ChangePoller::new();
        assert_eq!(poller.last_sequence(), 0);
    }

    #[test]
    fn test_change_poller_from_sequence() {
        let poller = ChangePoller::from_sequence(42);
        assert_eq!(poller.last_sequence(), 42);
    }

    #[test]
    fn test_change_poller_default() {
        let poller = ChangePoller::default();
        assert_eq!(poller.last_sequence(), 0);
    }

    #[test]
    fn test_change_poller_with_batch_limit() {
        let poller = ChangePoller::new().with_batch_limit(10);
        assert_eq!(poller.batch_limit, 10);
    }
}
