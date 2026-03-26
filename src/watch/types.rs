//! Public types for the watch system.
//!
//! These types define the event model for real-time experience notifications
//! and the [`WatchStream`] adapter that bridges sync crossbeam channels to
//! the async [`futures_core::Stream`] interface.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use atomic_waker::AtomicWaker;
use crossbeam_channel::Receiver;
use futures_core::Stream;

use crate::experience::ExperienceType;
use crate::storage::schema::{WatchEventRecord, WatchEventTypeTag};
use crate::types::{CollectiveId, ExperienceId, Timestamp};

/// An event emitted when an experience changes.
///
/// Watch events are delivered in-process via bounded crossbeam channels.
/// Each event identifies the experience, collective, mutation type, and
/// when it occurred.
///
/// # Example
///
/// ```rust,no_run
/// # #[tokio::main]
/// # async fn main() -> pulsedb::Result<()> {
/// # let dir = tempfile::tempdir().unwrap();
/// # let db = pulsedb::PulseDB::open(dir.path().join("test.db"), pulsedb::Config::default())?;
/// # let collective_id = db.create_collective("example")?;
/// use futures::StreamExt;
/// use pulsedb::WatchEventType;
///
/// let mut stream = db.watch_experiences(collective_id)?;
/// while let Some(event) = stream.next().await {
///     match event.event_type {
///         WatchEventType::Created => println!("New experience: {}", event.experience_id),
///         WatchEventType::Deleted => println!("Removed: {}", event.experience_id),
///         _ => {}
///     }
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct WatchEvent {
    /// The experience that changed.
    pub experience_id: ExperienceId,

    /// The collective the experience belongs to.
    pub collective_id: CollectiveId,

    /// What kind of change occurred.
    pub event_type: WatchEventType,

    /// When the change occurred.
    pub timestamp: Timestamp,
}

/// The kind of change that triggered a [`WatchEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchEventType {
    /// A new experience was recorded.
    Created,

    /// An existing experience was modified (fields updated or reinforced).
    Updated,

    /// An experience was soft-deleted (archived).
    Archived,

    /// An experience was permanently deleted.
    Deleted,
}

// ============================================================================
// Conversions between storage tags and public types
// ============================================================================

impl From<WatchEventType> for WatchEventTypeTag {
    fn from(value: WatchEventType) -> Self {
        match value {
            WatchEventType::Created => Self::Created,
            WatchEventType::Updated => Self::Updated,
            WatchEventType::Archived => Self::Archived,
            WatchEventType::Deleted => Self::Deleted,
        }
    }
}

impl From<WatchEventTypeTag> for WatchEventType {
    fn from(value: WatchEventTypeTag) -> Self {
        match value {
            WatchEventTypeTag::Created => Self::Created,
            WatchEventTypeTag::Updated => Self::Updated,
            WatchEventTypeTag::Archived => Self::Archived,
            WatchEventTypeTag::Deleted => Self::Deleted,
        }
    }
}

impl From<WatchEventRecord> for WatchEvent {
    fn from(record: WatchEventRecord) -> Self {
        Self {
            experience_id: ExperienceId::from_bytes(record.entity_id),
            collective_id: CollectiveId::from_bytes(record.collective_id),
            event_type: record.event_type.into(),
            timestamp: Timestamp::from_millis(record.timestamp_ms),
        }
    }
}

/// Filter for narrowing which watch events a subscriber receives.
///
/// All fields are optional. When multiple fields are set, they are combined
/// with AND logic: an event must match **all** specified criteria.
///
/// Filters are applied on the sender side before channel delivery, so
/// subscribers only receive events they care about.
///
/// # Example
///
/// ```rust
/// # fn main() -> pulsedb::Result<()> {
/// # let dir = tempfile::tempdir().unwrap();
/// # let db = pulsedb::PulseDB::open(dir.path().join("test.db"), pulsedb::Config::default())?;
/// # let collective_id = db.create_collective("example")?;
/// use pulsedb::WatchFilter;
///
/// let filter = WatchFilter {
///     domains: Some(vec!["security".to_string()]),
///     min_importance: Some(0.7),
///     ..Default::default()
/// };
/// let stream = db.watch_experiences_filtered(collective_id, filter)?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, Default)]
pub struct WatchFilter {
    /// Only emit events for experiences in these domains.
    /// If `None`, all domains match.
    pub domains: Option<Vec<String>>,

    /// Only emit events for these experience types.
    /// If `None`, all types match.
    pub experience_types: Option<Vec<ExperienceType>>,

    /// Only emit events for experiences with importance >= this threshold.
    /// If `None`, all importance levels match.
    pub min_importance: Option<f32>,
}

/// A stream of [`WatchEvent`] values backed by a crossbeam channel.
///
/// Implements [`futures_core::Stream`] for async consumption. The stream
/// yields `Some(event)` for each change and returns `None` when all
/// senders are dropped (database closed or subscriber removed).
///
/// Dropping a `WatchStream` automatically unregisters the subscriber
/// from the watch service, preventing memory leaks.
pub struct WatchStream {
    /// The receiving end of the crossbeam bounded channel.
    pub(crate) receiver: Receiver<WatchEvent>,

    /// Shared waker that the sender side calls `.wake()` on after `try_send`.
    pub(crate) waker: Arc<AtomicWaker>,

    /// Cleanup function called on drop to remove subscriber from registry.
    pub(crate) cleanup: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Stream for WatchStream {
    type Item = WatchEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Register the waker FIRST so we don't miss a wake between
        // try_recv and returning Pending.
        self.waker.register(cx.waker());

        match self.receiver.try_recv() {
            Ok(event) => Poll::Ready(Some(event)),
            Err(crossbeam_channel::TryRecvError::Empty) => Poll::Pending,
            Err(crossbeam_channel::TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}

impl Drop for WatchStream {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}

// Safety: crossbeam Receiver is Send, AtomicWaker is Send+Sync, cleanup is Send+Sync.
// WatchStream is not Sync (Receiver is not Sync), which is correct for Stream consumers.
impl std::fmt::Debug for WatchStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchStream")
            .field("pending_events", &self.receiver.len())
            .finish_non_exhaustive()
    }
}
