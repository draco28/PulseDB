//! Sync manager — orchestrates sync lifecycle between PulseDB instances.
//!
//! [`SyncManager`] is the public API for sync. It manages:
//! - Handshake negotiation with remote peer
//! - Background push/pull loops on configured intervals
//! - Manual one-shot sync via [`sync_once()`](SyncManager::sync_once)
//! - Initial catchup sync with progress callback
//! - Error recovery with exponential backoff
//! - Graceful shutdown

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, warn};

use crate::db::PulseDB;

use super::applier::RemoteChangeApplier;
use super::config::{SyncConfig, SyncDirection};
use super::error::SyncError;
use super::progress::SyncProgressCallback;
use super::pusher::LocalChangePusher;
use super::transport::SyncTransport;
use super::types::{
    HandshakeRequest, InstanceId, PullRequest, PullResponse, SyncPosition, SyncStats, SyncStatus,
};
use super::{SYNC_CAPABILITY_GCOUNTER_APPLICATIONS, SYNC_PROTOCOL_VERSION};

/// Orchestrator for sync operations between two PulseDB instances.
///
/// The SyncManager is a **sidecar** — it holds `Arc<PulseDB>` but doesn't
/// wrap it. Local database operations are completely unaffected by sync state.
///
/// # Peer identity is revalidated, not cached for the session
///
/// Every sync position is keyed on the peer's [`InstanceId`], so a manager that
/// trusted the handshake's answer for the life of the session would sync a
/// *different* peer against the previous one's cursors. That is not
/// hypothetical: [`PulseDB::remint_instance_id`] exists precisely to give a
/// restored file copy a fresh identity, so an endpoint restored from an older
/// snapshot comes back as a different peer holding **less** data.
///
/// Each pull therefore checks the identity the response reports against the
/// bound one, and a mismatch re-establishes the identity and switches to the
/// new peer's own cursors — absent meaning `0`, which re-pushes from the start.
/// A re-push of changes the peer already holds is absorbed by the applier's
/// idempotent skip path; skipping changes it is *missing* is silent data loss,
/// so `0` is the conservative answer for a peer whose contents cannot be known.
///
/// **The previous identity's cursor row is retained, never deleted.** Deleting
/// it would be a data decision this manager is not entitled to make: the old
/// identity may legitimately return (a rolled-back snapshot, a second replica
/// restored from a copy taken before the remint), and its row is the only
/// record of what that peer was sent. Retaining it is also the safe direction
/// for compaction — [`PulseDB::compact_wal`] takes the *minimum*
/// `push_sequence` over all known peers, so an extra row can only hold
/// compaction back, never release it.
///
/// **Limitation:** the peer's live identity reaches this side only on a pull
/// response, so a [`SyncDirection::PushOnly`](super::config::SyncDirection)
/// manager has nothing to revalidate against and keeps its handshake answer
/// until it is reconstructed. See [`SyncManager::peer_identity_change`].
///
/// # Example
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use pulsedb::{PulseDB, Config};
/// use pulsedb::sync::{SyncManager, SyncConfig, InMemorySyncTransport};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let db = Arc::new(PulseDB::open("my.db", Config::default())?);
/// let (local_transport, _remote) = InMemorySyncTransport::new_pair();
/// let mut manager = SyncManager::new(db, Box::new(local_transport), SyncConfig::default());
/// manager.start().await?;
/// // ... sync runs in background ...
/// manager.stop().await?;
/// # Ok(())
/// # }
/// ```
pub struct SyncManager {
    db: Arc<PulseDB>,
    transport: Arc<dyn SyncTransport>,
    config: SyncConfig,
    local_instance_id: InstanceId,
    /// The peer identity this session is bound to, or `None` before the first
    /// handshake.
    ///
    /// A **cache, revalidated on every pull** — never a fact fixed for the life
    /// of the manager. See [`SyncManager::peer_identity_change`] for the
    /// detection point and [`SyncManager::reestablish_peer`] for what a
    /// detected change does.
    peer_instance_id: Option<InstanceId>,
    status: Arc<RwLock<SyncStatus>>,
    stats: Arc<Mutex<SyncStats>>,
    shutdown: Arc<Notify>,
    task_handle: Option<JoinHandle<()>>,
}

impl SyncManager {
    /// Creates a new SyncManager.
    ///
    /// Does NOT start sync — call [`start()`](Self::start) or
    /// [`sync_once()`](Self::sync_once) to begin.
    pub fn new(db: Arc<PulseDB>, transport: Box<dyn SyncTransport>, config: SyncConfig) -> Self {
        // Read once, here: a `PulseDB::remint_instance_id()` after this point
        // is not observed by this manager (documented pre-manager rule).
        let local_instance_id = db.instance_id();
        Self {
            db,
            transport: Arc::from(transport),
            config,
            local_instance_id,
            peer_instance_id: None,
            status: Arc::new(RwLock::new(SyncStatus::Idle)),
            stats: Arc::new(Mutex::new(SyncStats::default())),
            shutdown: Arc::new(Notify::new()),
            task_handle: None,
        }
    }

    /// Starts the background sync loop.
    ///
    /// Performs a handshake with the remote peer, then spawns a background
    /// tokio task that pushes and pulls on the configured intervals.
    #[instrument(skip(self), fields(instance_id = %self.local_instance_id))]
    pub async fn start(&mut self) -> Result<(), SyncError> {
        if self.task_handle.is_some() {
            return Err(SyncError::transport("SyncManager already started"));
        }

        // Perform handshake
        let peer_id = self.perform_handshake().await?;
        self.peer_instance_id = Some(peer_id);

        self.set_status(SyncStatus::Syncing);

        // Clone everything needed for the background task
        let db = Arc::clone(&self.db);
        let transport = Arc::clone(&self.transport);
        let config = self.config.clone();
        let local_id = self.local_instance_id;
        let status = Arc::clone(&self.status);
        let stats = Arc::clone(&self.stats);
        let shutdown = Arc::clone(&self.shutdown);

        let handle = tokio::spawn(async move {
            Self::background_loop(
                db, transport, config, local_id, peer_id, status, stats, shutdown,
            )
            .await;
        });

        self.task_handle = Some(handle);
        info!("SyncManager started");
        Ok(())
    }

    /// Stops the background sync loop.
    #[instrument(skip(self))]
    pub async fn stop(&mut self) -> Result<(), SyncError> {
        if let Some(handle) = self.task_handle.take() {
            self.shutdown.notify_one();
            handle
                .await
                .map_err(|e| SyncError::transport(format!("Background task panicked: {}", e)))?;
            self.set_status(SyncStatus::Idle);
            info!("SyncManager stopped");
        }
        Ok(())
    }

    /// Performs a single pull+push sync cycle (no background task needed).
    ///
    /// Useful for testing or manual sync triggers.
    ///
    /// # Ordering
    ///
    /// The pull runs **first**, and that is load-bearing rather than
    /// incidental. The pull response is the only place the peer's live
    /// identity reaches this side (see
    /// [`peer_identity_change`](Self::peer_identity_change)), and every
    /// position this cycle would persist — the pusher's acknowledgement
    /// included — is derived from the identity it is keyed on. Pulling first
    /// means the identity is *confirmed before anything is written*: a cycle
    /// that discovers the peer reminted persists nothing at all, re-establishes
    /// the identity, and then runs the whole cycle again against the new
    /// peer's own cursors. Pushing first would file an acknowledgement produced
    /// by the *new* peer under the *old* peer's key before the pull could
    /// notice — a position that peer never acknowledged, which
    /// [`PulseDB::compact_wal`] would then trust.
    #[instrument(skip(self))]
    pub async fn sync_once(&mut self) -> Result<SyncStatus, SyncError> {
        let mut peer_id = self.bound_peer().await?;

        self.set_status(SyncStatus::Syncing);

        let applier = RemoteChangeApplier::new(Arc::clone(&self.db), self.config.clone());

        // Pull if enabled. On a detected remint this re-establishes the
        // identity and pulls again from the NEW peer's cursor; `peer_id` is
        // rebound in place so the push below is keyed on it too.
        let pulled = if matches!(
            self.config.direction,
            SyncDirection::PullOnly | SyncDirection::Bidirectional
        ) {
            self.pull_and_apply(&applier, &mut peer_id).await?
        } else {
            0
        };

        // Push if enabled. Built AFTER the pull so the push position it resumes
        // from, and the acknowledgement it persists, belong to the identity the
        // pull just confirmed — an unseen identity starts at 0 and re-sends.
        let pushed = if matches!(
            self.config.direction,
            SyncDirection::PushOnly | SyncDirection::Bidirectional
        ) {
            let push_seq = Self::load_push_sequence(&self.db, peer_id)?;
            let mut pusher = LocalChangePusher::new(
                Arc::clone(&self.db),
                Arc::clone(&self.transport),
                self.config.clone(),
                self.local_instance_id,
                peer_id,
                push_seq,
            );
            pusher.push_pending().await?
        } else {
            0
        };

        self.set_status(SyncStatus::Idle);

        debug!(pushed, pulled, "sync_once complete");
        Ok(SyncStatus::Idle)
    }

    /// Performs initial sync — pulls all remote changes in batches.
    ///
    /// Call this before `start()` to catch up from a cold start.
    ///
    /// # Errors
    ///
    /// `Ok(())` means the catch-up COMPLETED: a page the peer reported as its
    /// last was pulled, and every change in the run applied or was idempotently
    /// skipped. Anything short of that is
    /// [`SyncError::CatchUpIncomplete`] — the peer stalled (it reported more
    /// changes while handing back an unadvanced cursor, the shape a
    /// fully-filtered server page produces), or a change was left unapplied.
    /// Both leave the pull position persisted where the run stopped, so a later
    /// `initial_sync` or a background cycle resumes from there; neither is a
    /// reason to discard local state. Transport and handshake failures surface
    /// as their own variants, as before.
    ///
    /// "Left unapplied" is about the END of the run, not about attempts made
    /// along the way. This loop retries: an apply failure holds the position
    /// strictly below the failing sequence, so the next iteration re-requests
    /// that change, and a transient failure — a storage error, a contended lock
    /// — applies on the retry. Only a sequence still ABOVE the final pull
    /// position was never applied; the position is inclusive, so one at or
    /// below it was handled (and is a sequence this cursor would never fetch
    /// again in any case). A catch-up that stumbled and recovered reports
    /// `Ok(())`.
    ///
    /// This strictness is specific to `initial_sync`, a one-shot "catch me up"
    /// call whose `Ok` a caller is entitled to trust. A single background cycle
    /// ([`sync_once`](Self::sync_once)) still returns `Ok` with a change left
    /// unapplied — there the next cycle retries it, which is the design.
    #[instrument(skip(self, progress))]
    pub async fn initial_sync(
        &mut self,
        progress: Option<Box<dyn SyncProgressCallback>>,
    ) -> Result<(), SyncError> {
        let mut peer_id = self.bound_peer().await?;

        self.set_status(SyncStatus::Syncing);

        let applier = RemoteChangeApplier::new(Arc::clone(&self.db), self.config.clone());

        let mut total_pulled = 0usize;
        let mut position = SyncPosition::new(peer_id, Self::load_pull_sequence(&self.db, peer_id)?);
        // The SEQUENCES of the changes that ERRORED (never the idempotent
        // skips, which are ordinary successful re-sync outcomes — see
        // `ApplyResult::failed`). A catch-up that left one of these behind did
        // not catch up, whatever the peer said about further pages.
        //
        // Sequences, not a count, because a failure here is an ATTEMPT and this
        // loop retries: `safe_through` stops the position strictly below the
        // batch's lowest failure, so the next iteration re-requests from there
        // and receives the failed change again. A transient failure — a storage
        // error, a contended lock — then applies, and a count carried from the
        // first attempt would report a catch-up that in fact completed as
        // incomplete. Which sequences failed is resolved at termination against
        // the final position (below).
        let mut failed_sequences: BTreeSet<u64> = BTreeSet::new();
        // Set when the loop stops on an unadvanced position that the peer
        // claimed to have more changes beyond.
        let mut stalled_with_more = false;
        // A remint mid-catch-up is re-established ONCE. A peer whose identity
        // changes again in the same run is flapping (or the endpoint is two
        // instances behind one address), and re-establishing on every answer
        // would be an unbounded loop of handshakes that never catches up.
        let mut reestablished = false;

        loop {
            let pull_request = PullRequest {
                cursor: position.clone(),
                batch_size: self.config.batch_size,
                collectives: self.config.collectives.clone(),
            };

            let response = self.transport.pull_changes(pull_request).await?;

            // Identity FIRST — before a change is applied, before a position is
            // written. This page was requested from the OLD peer's position, so
            // everything in it is derived from an identity that turned out to be
            // stale: applying it would advance a cursor over sequences in a WAL
            // this manager has never read from position 0. Discard the page
            // whole, re-establish, and restart from the NEW peer's own cursor.
            if let Some(observed) = Self::peer_identity_change(peer_id, &response) {
                if reestablished {
                    return Err(SyncError::handshake(format!(
                        "peer identity changed twice during one initial_sync \
                         (bound {peer_id}, now {observed}); refusing to keep \
                         re-establishing"
                    )));
                }
                reestablished = true;
                peer_id = Self::reestablish_peer(
                    &self.transport,
                    self.local_instance_id,
                    peer_id,
                    observed,
                )
                .await?;
                self.peer_instance_id = Some(peer_id);
                position = SyncPosition::new(peer_id, Self::load_pull_sequence(&self.db, peer_id)?);
                // The failures recorded so far name sequences in the OLD peer's
                // WAL; carried into the new peer's sequence space they would be
                // compared against a position that has nothing to do with them.
                failed_sequences.clear();
                stalled_with_more = false;
                continue;
            }

            let batch_size = response.changes.len();
            let requested = position.sequence;

            // Advance only as far as the applier actually got. `apply_batch`
            // records a per-change failure instead of returning an error, so
            // storing the server's position would step over a change that was
            // never applied and never pull it again.
            let next = if batch_size > 0 {
                let result = applier.apply_batch(response.changes)?;
                result.record_into(&self.stats);
                failed_sequences.extend(result.failed_sequences.iter().copied());
                result.safe_through.unwrap_or(requested)
            } else {
                response.new_cursor.sequence
            };

            total_pulled += batch_size;
            // Keyed on the identity the check above just confirmed, NOT on
            // `response.new_cursor.instance_id` taken on trust: the two are
            // equal here by construction, and reaching for the response's copy
            // is how a position ends up filed under an identity the run never
            // requested from.
            position = SyncPosition::new(peer_id, next);

            // Save the pull position after each batch (crash-safe). Pull side
            // only — the push position is never touched here.
            Self::save_pull_position(&self.db, &position)?;

            if let Some(ref cb) = progress {
                cb.on_progress(batch_size, total_pulled, response.has_more);
            }

            // The position did not move, so the next request would be
            // byte-identical to this one. Three ways to get here:
            //
            // - the server has nothing at or after this position — the ordinary
            //   "already caught up" end of an initial sync (empty batch,
            //   `has_more: false`), which is not a fault and is not logged;
            // - a batch whose FIRST change failed to apply, which leaves the
            //   position exactly where it was; and
            // - an EMPTY batch reported with `has_more: true` — what the server
            //   returns when it filtered every event it polled (a `collectives`
            //   filter, or entities deleted since the WAL event so
            //   `build_change_from_record` yields `None`).
            //
            // The last two spin forever without this guard. An honest server
            // never advances `new_cursor.sequence` on an empty batch
            // (`SyncServer::handle_pull` and `InMemorySyncTransport::pull_changes`
            // both echo the requested sequence), so `next <= requested` can only
            // mean no progress. Stop and let the next sync retry from here.
            if next <= requested {
                if batch_size > 0 || response.has_more {
                    warn!(
                        peer = %peer_id,
                        position = requested,
                        batch_size,
                        has_more = response.has_more,
                        "Initial sync stopped: the pull position did not advance \
                         (the batch could not be applied past its first change, or \
                         the server returned no changes at this position)"
                    );
                }
                stalled_with_more = response.has_more;
                break;
            }

            if !response.has_more {
                break;
            }
        }

        // The status transition is the same one the success path makes: the
        // manager is no longer syncing either way, and a stopped catch-up must
        // not leave it wedged in `Syncing`. The error is what tells the caller
        // what happened. (Transport failures above still return through `?`
        // without a transition — pre-existing, untouched here.)
        self.set_status(SyncStatus::Idle);

        // Stopping short is not completion. Both shapes leave the pull position
        // persisted where the run stopped, so a retry resumes from there.
        //
        // A failure only counts if it is STILL UNRESOLVED here. The pull
        // position is INCLUSIVE — it is a `safe_through`, the highest sequence
        // at or below which every change was applied, resolved or idempotently
        // skipped — so a failed sequence at or below where the run ended was
        // handled by a later attempt in this same run, and is also a sequence
        // this cursor will never fetch again. Only `sequence > position` is
        // outstanding.
        let unresolved = failed_sequences
            .iter()
            .filter(|sequence| **sequence > position.sequence)
            .count();
        if unresolved > 0 {
            return Err(SyncError::catch_up_apply_failed(
                peer_id,
                position.sequence,
                unresolved,
            ));
        }
        if stalled_with_more {
            return Err(SyncError::catch_up_stalled(peer_id, position.sequence));
        }

        info!(total_pulled, "Initial sync complete");
        Ok(())
    }

    /// Returns the current sync status.
    pub fn status(&self) -> SyncStatus {
        self.status
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Returns a snapshot of the local-only sync counters accumulated over
    /// every change this manager has applied (`sync_once`, `initial_sync`
    /// and the background loop alike).
    ///
    /// See [`SyncStats::skewed_timestamps`] for the #13 skew counter.
    pub fn stats(&self) -> SyncStats {
        *self.stats.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ─── Internal helpers ────────────────────────────────────────────

    /// The peer identity this session is bound to, handshaking once if it has
    /// none yet.
    async fn bound_peer(&mut self) -> Result<InstanceId, SyncError> {
        match self.peer_instance_id {
            Some(peer_id) => Ok(peer_id),
            None => {
                let peer_id = self.perform_handshake().await?;
                self.peer_instance_id = Some(peer_id);
                Ok(peer_id)
            }
        }
    }

    /// The identity a pull response reports, when it is **not** the one this
    /// session is bound to.
    ///
    /// # Why the pull response, and only the pull response
    ///
    /// Three messages could in principle carry the peer's identity, and only
    /// one of them actually does so on every cycle:
    ///
    /// - **The handshake** carries it, but is performed once. Repeating it
    ///   every cycle to re-check would spend a round trip per cycle on an
    ///   answer that almost never changes, which is why the binding is cached
    ///   at all.
    /// - **[`PushResponse`](super::types::PushResponse)`::new_cursor` cannot be
    ///   used.** `SyncServer::handle_push` (`sync-http`)
    ///   fills it with `changes.first().source_instance` — the **sender's** id,
    ///   i.e. *ours* — because the position it acknowledges is a position in the
    ///   sender's WAL. `InMemorySyncTransport` fills the same field with the
    ///   peer's id instead. The two disagree, and against a real server the
    ///   comparison would report "identity changed" on every single push.
    /// - **[`PullResponse`]`::new_cursor.instance_id` is the peer's own id in
    ///   both implementations** — `SyncServer::handle_pull` writes
    ///   `self.instance_id`, `InMemorySyncTransport` writes its
    ///   `peer_instance_id` — because a pull position is a position in the
    ///   *peer's* WAL and is meaningless without saying whose. It arrives on
    ///   every pull, empty batches included, and a peer cannot answer a pull
    ///   without filling it in. That is the detection point.
    ///
    /// No wire change: the field is already there and already required to mean
    /// this. Protocol v4 bytes are untouched.
    ///
    /// # Cost on the ordinary path
    ///
    /// One [`InstanceId`] equality comparison per pull response. No extra
    /// request, no extra round trip, no extra storage read, and no additional
    /// handshake unless the comparison actually fails — the cache still holds
    /// for the life of the session when the peer's identity is stable.
    fn peer_identity_change(bound: InstanceId, response: &PullResponse) -> Option<InstanceId> {
        let observed = response.new_cursor.instance_id;
        (observed != bound).then_some(observed)
    }

    /// Re-establishes the peer identity after a pull answered under a different
    /// one, and returns the identity to use from here on.
    ///
    /// The observed id is treated as *evidence that the binding is stale*, not
    /// as the new binding: the identity is re-negotiated through a full
    /// handshake, so the protocol-version check runs again and the answer comes
    /// from the same exchange that established the first one. Whatever the
    /// handshake returns wins.
    ///
    /// The caller must have persisted **nothing** from the response that
    /// triggered this, and must reload its cursors for the returned identity —
    /// an absent cursor means position `0`.
    ///
    /// The previous identity's cursor row is left exactly as it was. See the
    /// [type-level docs](SyncManager) for why it is retained rather than
    /// deleted.
    async fn reestablish_peer(
        transport: &Arc<dyn SyncTransport>,
        local_instance_id: InstanceId,
        previous: InstanceId,
        observed: InstanceId,
    ) -> Result<InstanceId, SyncError> {
        warn!(
            previous = %previous,
            observed = %observed,
            "Sync peer identity changed (a remint, or a different instance behind \
             the same endpoint); re-handshaking and switching to the new peer's own cursor"
        );

        let peer_id = Self::handshake_with(transport, local_instance_id).await?;
        if peer_id != observed {
            debug!(
                observed = %observed,
                negotiated = %peer_id,
                "Re-handshake returned an identity other than the one the pull reported"
            );
        }
        Ok(peer_id)
    }

    #[instrument(skip(self))]
    async fn perform_handshake(&self) -> Result<InstanceId, SyncError> {
        Self::handshake_with(&self.transport, self.local_instance_id).await
    }

    /// The handshake proper, reachable without `&self`.
    ///
    /// The background task holds no `&self`, and an identity re-established
    /// there must go through exactly the same negotiation — protocol-version
    /// check included — as the one `start()` performed.
    async fn handshake_with(
        transport: &Arc<dyn SyncTransport>,
        local_instance_id: InstanceId,
    ) -> Result<InstanceId, SyncError> {
        let request = HandshakeRequest {
            instance_id: local_instance_id,
            protocol_version: SYNC_PROTOCOL_VERSION,
            capabilities: vec![
                "push".into(),
                "pull".into(),
                SYNC_CAPABILITY_GCOUNTER_APPLICATIONS.into(),
            ],
        };

        let response = transport.handshake(request).await?;

        // Protocol-version check FIRST. A server that speaks a different
        // version answers with the soft `accepted: false` path carrying its
        // own version (see `SyncServer::handle_handshake`), so mapping the
        // rejection generically before this check would shadow the typed
        // variant behind a reason string (#12). Callers can now match on
        // `SyncError::ProtocolVersion { local, remote }`.
        if response.protocol_version != SYNC_PROTOCOL_VERSION {
            warn!(
                peer = %response.instance_id,
                local = SYNC_PROTOCOL_VERSION,
                remote = response.protocol_version,
                "Sync handshake refused: protocol version mismatch"
            );
            return Err(SyncError::ProtocolVersion {
                local: SYNC_PROTOCOL_VERSION,
                remote: response.protocol_version,
            });
        }

        if !response.accepted {
            return Err(SyncError::handshake(
                response.reason.unwrap_or_else(|| "rejected".into()),
            ));
        }

        debug!(peer = %response.instance_id, "Handshake accepted");
        Ok(response.instance_id)
    }

    fn set_status(&self, status: SyncStatus) {
        if let Ok(mut s) = self.status.write() {
            *s = status;
        }
    }

    /// The persisted push position for `peer_id` (0 if no cursor yet).
    ///
    /// An absent cursor is `0` — start of the WAL — which is the conservative
    /// answer for an identity this store has never synced with, a peer restored
    /// from an older snapshot included: re-sending changes it already holds is
    /// absorbed by the applier's idempotent skip path, while skipping changes it
    /// is missing is silent data loss.
    fn load_push_sequence(db: &PulseDB, peer_id: InstanceId) -> Result<u64, SyncError> {
        db.storage_for_test()
            .load_sync_cursor(&peer_id)
            .map_err(|e| SyncError::transport(format!("Failed to load cursor: {}", e)))
            .map(|opt| opt.map_or(0, |c| c.push_sequence))
    }

    /// The persisted pull position for `peer_id` (0 if no cursor yet).
    fn load_pull_sequence(db: &PulseDB, peer_id: InstanceId) -> Result<u64, SyncError> {
        db.storage_for_test()
            .load_sync_cursor(&peer_id)
            .map_err(|e| SyncError::transport(format!("Failed to load cursor: {}", e)))
            .map(|opt| opt.map_or(0, |c| c.pull_sequence))
    }

    /// Persists a pull position **under the identity the position itself
    /// names** (pull side only).
    ///
    /// Taking the whole [`SyncPosition`] rather than an id and a sequence is
    /// the point: a position and the key it is filed under can no longer be
    /// supplied separately, so filing one peer's position under another peer's
    /// row is not an argument a caller can get wrong. That was the corruption
    /// half of the cached-identity defect — a pull response carrying a
    /// *reminted* peer's `instance_id` had its sequence written under the id
    /// the handshake had returned earlier, leaving the store wrong for both
    /// identities.
    fn save_pull_position(db: &PulseDB, position: &SyncPosition) -> Result<(), SyncError> {
        db.storage_for_test()
            .update_pull_cursor(&position.instance_id, position.sequence)
            .map_err(|e| SyncError::transport(format!("Failed to save pull cursor: {}", e)))
    }

    /// One pull: request from `peer_id`'s stored position, apply, persist.
    ///
    /// Returns [`PullOutcome::PeerChanged`] **without applying or persisting
    /// anything** when the response answers under another identity — see
    /// [`peer_identity_change`](Self::peer_identity_change). The batch is
    /// discarded rather than re-attributed: it was requested from the old
    /// peer's position, so its sequences belong to a WAL this manager has never
    /// read from the start, and storing where it reached would claim a
    /// catch-up that never happened.
    async fn pull_step(
        db: &Arc<PulseDB>,
        transport: &Arc<dyn SyncTransport>,
        applier: &RemoteChangeApplier,
        config: &SyncConfig,
        peer_id: InstanceId,
        stats: &Mutex<SyncStats>,
    ) -> Result<PullOutcome, SyncError> {
        let pull_seq = Self::load_pull_sequence(db, peer_id)?;
        let pull_request = PullRequest {
            cursor: SyncPosition::new(peer_id, pull_seq),
            batch_size: config.batch_size,
            collectives: config.collectives.clone(),
        };

        let response = transport.pull_changes(pull_request).await?;

        if let Some(observed) = Self::peer_identity_change(peer_id, &response) {
            return Ok(PullOutcome::PeerChanged(observed));
        }

        let count = response.changes.len();

        // Advance only as far as the applier got (see `initial_sync`): a change
        // that failed to apply must stay ahead of the stored position so the
        // next pull fetches it again.
        let next = if count > 0 {
            let result = applier.apply_batch(response.changes)?;
            result.record_into(stats);
            result.safe_through.unwrap_or(pull_seq)
        } else {
            response.new_cursor.sequence
        };
        // Persist on EVERY successful pull, empty batches included. The record
        // is what represents this peer in the cursor store, and `compact_wal`
        // needs to see its `push_sequence == 0` to stay blocked; a PullOnly
        // peer that never returns changes would otherwise have no record at
        // all, and compaction would delete events it has never been sent.
        Self::save_pull_position(db, &SyncPosition::new(peer_id, next))?;

        Ok(PullOutcome::Applied(count))
    }

    /// Pull changes from remote and apply them locally, re-establishing the
    /// peer identity and pulling again if the peer answered under a different
    /// one.
    ///
    /// `peer_id` is rebound in place, so the caller's later work in the same
    /// cycle — the push, in particular — is keyed on the confirmed identity.
    async fn pull_and_apply(
        &mut self,
        applier: &RemoteChangeApplier,
        peer_id: &mut InstanceId,
    ) -> Result<usize, SyncError> {
        let first = Self::pull_step(
            &self.db,
            &self.transport,
            applier,
            &self.config,
            *peer_id,
            &self.stats,
        )
        .await?;

        let observed = match first {
            PullOutcome::Applied(count) => return Ok(count),
            PullOutcome::PeerChanged(observed) => observed,
        };

        *peer_id =
            Self::reestablish_peer(&self.transport, self.local_instance_id, *peer_id, observed)
                .await?;
        self.peer_instance_id = Some(*peer_id);

        // Retry from the NEW identity's own cursor — the first thing this cycle
        // persists is therefore derived from that identity's position, never
        // from the stale one's. Exactly one retry: an identity that changes
        // again in the same cycle is flapping, and re-establishing on every
        // answer would loop.
        match Self::pull_step(
            &self.db,
            &self.transport,
            applier,
            &self.config,
            *peer_id,
            &self.stats,
        )
        .await?
        {
            PullOutcome::Applied(count) => Ok(count),
            PullOutcome::PeerChanged(again) => Err(SyncError::handshake(format!(
                "peer identity changed twice during one sync cycle \
                 (bound {peer_id}, now {again}); refusing to keep re-establishing"
            ))),
        }
    }

    /// Background loop that runs push+pull on configured intervals.
    #[allow(clippy::too_many_arguments)]
    async fn background_loop(
        db: Arc<PulseDB>,
        transport: Arc<dyn SyncTransport>,
        config: SyncConfig,
        local_id: InstanceId,
        peer_id: InstanceId,
        status: Arc<RwLock<SyncStatus>>,
        stats: Arc<Mutex<SyncStats>>,
        shutdown: Arc<Notify>,
    ) {
        // Rebound in place by `run_sync_cycle` when a pull reveals that the
        // endpoint is answering under a different identity.
        let mut peer_id = peer_id;

        let interval_ms = std::cmp::max(config.push_interval_ms, config.pull_interval_ms);
        let interval = tokio::time::Duration::from_millis(interval_ms);

        let mut consecutive_failures = 0u32;
        let max_retries = config.retry.max_retries;
        let initial_backoff = config.retry.initial_backoff_ms;
        let max_backoff = config.retry.max_backoff_ms;
        let multiplier = config.retry.backoff_multiplier;

        loop {
            let sleep_duration = if consecutive_failures > 0 {
                // Exponential backoff
                let backoff = (initial_backoff as f64)
                    * multiplier.powi(consecutive_failures.saturating_sub(1) as i32);
                let backoff_ms = (backoff as u64).min(max_backoff);
                tokio::time::Duration::from_millis(backoff_ms)
            } else {
                interval
            };

            tokio::select! {
                _ = shutdown.notified() => {
                    debug!("Sync background loop shutting down");
                    break;
                }
                _ = tokio::time::sleep(sleep_duration) => {
                    let applier = RemoteChangeApplier::new(Arc::clone(&db), config.clone());

                    // `peer_id` is a `mut` local, not the value captured at
                    // `start()`: a cycle that detects a remint rebinds it here,
                    // and every later cycle loads ITS cursors. Nothing about the
                    // identity is fixed for the life of the task.
                    let result = Self::run_sync_cycle(
                        &applier, &transport, &db, &config, local_id, &mut peer_id, &stats,
                    )
                    .await;

                    match result {
                        Ok(_) => {
                            if consecutive_failures > 0 {
                                info!("Sync recovered after {} failures", consecutive_failures);
                            }
                            consecutive_failures = 0;
                            if let Ok(mut s) = status.write() {
                                *s = SyncStatus::Syncing;
                            }
                        }
                        Err(e) => {
                            consecutive_failures += 1;
                            if consecutive_failures > max_retries {
                                warn!(
                                    failures = consecutive_failures,
                                    "Sync errors exceed max_retries, continuing with backoff"
                                );
                            }
                            error!("Sync cycle failed: {}", e);
                            if let Ok(mut s) = status.write() {
                                *s = SyncStatus::Error(e.to_string());
                            }
                        }
                    }
                }
            }
        }

        if let Ok(mut s) = status.write() {
            *s = SyncStatus::Idle;
        }
    }

    /// Execute one pull+push cycle. Used by the background loop.
    ///
    /// Same ordering, and for the same reason, as
    /// [`sync_once`](Self::sync_once): the pull confirms the peer's identity
    /// before the pusher is built from — and acknowledges into — that
    /// identity's cursor row.
    ///
    /// `peer_id` is rebound **in place** when the pull reveals a different
    /// peer, which is what keeps the background task from running the rest of
    /// its life against the identity it was spawned with. On a detected change
    /// this cycle does no push at all: the next tick runs a whole clean cycle
    /// against the new identity's cursors, which for an unseen peer means
    /// pushing from `0`.
    ///
    /// The manager's own `peer_instance_id` is not written from here — the task
    /// holds no `&mut self`. A [`sync_once`](Self::sync_once) issued after a
    /// [`stop()`](Self::stop) therefore starts from the identity
    /// [`start()`](Self::start) bound and re-detects on its own first pull,
    /// which costs one cycle and cannot lose data.
    #[allow(clippy::too_many_arguments)]
    async fn run_sync_cycle(
        applier: &RemoteChangeApplier,
        transport: &Arc<dyn SyncTransport>,
        db: &Arc<PulseDB>,
        config: &SyncConfig,
        local_id: InstanceId,
        peer_id: &mut InstanceId,
        stats: &Mutex<SyncStats>,
    ) -> Result<(), SyncError> {
        // Pull
        if matches!(
            config.direction,
            SyncDirection::PullOnly | SyncDirection::Bidirectional
        ) {
            match Self::pull_step(db, transport, applier, config, *peer_id, stats).await? {
                PullOutcome::Applied(_) => {}
                PullOutcome::PeerChanged(observed) => {
                    *peer_id =
                        Self::reestablish_peer(transport, local_id, *peer_id, observed).await?;
                    // Nothing was applied and nothing persisted. Skip the push
                    // too — it would be built from a cursor this cycle has just
                    // learned belongs to a peer that is no longer there.
                    return Ok(());
                }
            }
        }

        // Push, from the position the confirmed identity has acknowledged.
        if matches!(
            config.direction,
            SyncDirection::PushOnly | SyncDirection::Bidirectional
        ) {
            let push_seq = Self::load_push_sequence(db, *peer_id).unwrap_or(0);
            let mut pusher = LocalChangePusher::new(
                Arc::clone(db),
                Arc::clone(transport),
                config.clone(),
                local_id,
                *peer_id,
                push_seq,
            );
            pusher.push_pending().await?;
        }

        Ok(())
    }
}

/// What a single [`SyncManager::pull_step`] observed.
enum PullOutcome {
    /// The peer answered under the bound identity; the batch was applied and
    /// the pull position persisted. Carries the number of changes received.
    Applied(usize),

    /// The peer answered under a **different** instance id, carried here.
    ///
    /// Nothing was applied and nothing was persisted — the response was
    /// derived from the previous identity's position, so neither its changes
    /// nor its cursor may be attributed to either peer.
    PeerChanged(InstanceId),
}
