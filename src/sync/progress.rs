//! Progress callback for initial sync operations.
//!
//! Consumers implement [`SyncProgressCallback`] to receive progress updates
//! during [`SyncManager::initial_sync()`](super::manager::SyncManager::initial_sync),
//! typically for driving a loading bar in the UI.

/// Callback for reporting sync progress during initial catchup.
///
/// Implement this trait to receive updates as batches of changes are
/// pulled and applied during initial sync.
///
/// # Example
///
/// ```rust
/// use pulsedb::sync::progress::SyncProgressCallback;
///
/// struct ProgressBar;
///
/// impl SyncProgressCallback for ProgressBar {
///     fn on_progress(&self, batch_complete: usize, total_pulled: usize, has_more: bool) {
///         println!("Pulled {} changes (batch of {}), more: {}", total_pulled, batch_complete, has_more);
///     }
/// }
/// ```
pub trait SyncProgressCallback: Send {
    /// Called after each batch of changes is pulled and applied.
    ///
    /// # Arguments
    ///
    /// * `batch_complete` — Number of changes in the batch just applied
    /// * `total_pulled` — Cumulative count of all changes pulled so far
    /// * `has_more` — Whether the remote has more changes to send
    fn on_progress(&self, batch_complete: usize, total_pulled: usize, has_more: bool);
}
