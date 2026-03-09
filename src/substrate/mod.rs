//! SubstrateProvider async trait for PulseHive integration.
//!
//! This module defines the async interface between PulseDB (the storage layer)
//! and PulseHive (the intelligence layer). PulseHive's `HiveMind` holds a
//! `Box<dyn SubstrateProvider>` to interact with the database without knowing
//! the concrete storage implementation.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────┐       ┌──────────────────────┐
//! │     PulseHive        │       │       PulseDB         │
//! │                      │       │                       │
//! │  HiveMind ───────────┼──────►│  PulseDBSubstrate     │
//! │  Box<dyn Substrate>  │       │  (Arc<PulseDB>)       │
//! │                      │       │                       │
//! │  Agents perceive     │       │  spawn_blocking ──►   │
//! │  the field through   │       │  sync storage ops     │
//! │  SubstrateProvider   │       │                       │
//! └──────────────────────┘       └──────────────────────┘
//! ```
//!
//! # Example
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use pulsedb::{PulseDB, Config, PulseDBSubstrate, SubstrateProvider};
//!
//! // Create PulseDB and wrap in substrate
//! let db = Arc::new(PulseDB::open("./pulse.db", Config::default())?);
//! let substrate = PulseDBSubstrate::new(db);
//!
//! // Use as trait object (how PulseHive consumes it)
//! let provider: Box<dyn SubstrateProvider> = Box::new(substrate);
//!
//! // All operations are async
//! let exp_id = provider.store_experience(new_exp).await?;
//! let results = provider.search_similar(collective, &embedding, 10).await?;
//! ```

mod r#impl;

pub use r#impl::PulseDBSubstrate;

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;

use crate::activity::Activity;
use crate::error::PulseDBError;
use crate::experience::{Experience, NewExperience};
use crate::insight::{DerivedInsight, NewDerivedInsight};
use crate::relation::{ExperienceRelation, NewExperienceRelation};
use crate::search::{ContextCandidates, ContextRequest};
use crate::types::{CollectiveId, ExperienceId, InsightId, RelationId};
use crate::watch::WatchEvent;

/// Async storage interface for PulseHive integration.
///
/// This trait abstracts PulseDB's storage capabilities behind an async
/// boundary, enabling PulseHive's `HiveMind` to interact with the database
/// without blocking the async runtime.
///
/// # Object Safety
///
/// `SubstrateProvider` is object-safe via `#[async_trait]`, allowing it to
/// be used as `Box<dyn SubstrateProvider>` in PulseHive.
///
/// # Implementors
///
/// - [`PulseDBSubstrate`] — production implementation wrapping `Arc<PulseDB>`
#[async_trait]
pub trait SubstrateProvider: Send + Sync {
    /// Stores a new experience and returns its assigned ID.
    ///
    /// Generates an embedding (if configured), writes to storage, and
    /// indexes in the collective's HNSW graph.
    async fn store_experience(&self, exp: NewExperience) -> Result<ExperienceId, PulseDBError>;

    /// Retrieves an experience by ID, or `None` if it doesn't exist.
    async fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>, PulseDBError>;

    /// Searches for experiences similar to the given embedding.
    ///
    /// Returns up to `k` results as `(Experience, similarity_score)` tuples,
    /// sorted by similarity descending (1.0 = identical).
    async fn search_similar(
        &self,
        collective: CollectiveId,
        embedding: &[f32],
        k: usize,
    ) -> Result<Vec<(Experience, f32)>, PulseDBError>;

    /// Retrieves the most recent experiences from a collective.
    ///
    /// Returns up to `limit` experiences sorted by timestamp descending.
    async fn get_recent(
        &self,
        collective: CollectiveId,
        limit: usize,
    ) -> Result<Vec<Experience>, PulseDBError>;

    /// Stores a relation between two experiences.
    async fn store_relation(&self, rel: NewExperienceRelation) -> Result<RelationId, PulseDBError>;

    /// Retrieves all experiences related to the given experience (both directions).
    ///
    /// Returns `(related_experience, relation)` tuples for both outgoing
    /// and incoming relations.
    async fn get_related(
        &self,
        exp_id: ExperienceId,
    ) -> Result<Vec<(Experience, ExperienceRelation)>, PulseDBError>;

    /// Stores a derived insight synthesized from source experiences.
    async fn store_insight(&self, insight: NewDerivedInsight) -> Result<InsightId, PulseDBError>;

    /// Searches for insights similar to the given embedding.
    ///
    /// Returns up to `k` results as `(DerivedInsight, similarity_score)` tuples.
    async fn get_insights(
        &self,
        collective: CollectiveId,
        embedding: &[f32],
        k: usize,
    ) -> Result<Vec<(DerivedInsight, f32)>, PulseDBError>;

    /// Retrieves active (non-stale) agent activities in a collective.
    async fn get_activities(&self, collective: CollectiveId)
        -> Result<Vec<Activity>, PulseDBError>;

    /// Assembles context candidates from all retrieval primitives.
    ///
    /// Orchestrates similarity search, recent experiences, insights,
    /// relations, and active agents into a single response.
    async fn get_context_candidates(
        &self,
        request: ContextRequest,
    ) -> Result<ContextCandidates, PulseDBError>;

    /// Subscribes to real-time experience change events in a collective.
    ///
    /// Returns a `Stream` that yields [`WatchEvent`] values whenever
    /// experiences are created, updated, archived, or deleted.
    async fn watch(
        &self,
        collective: CollectiveId,
    ) -> Result<Pin<Box<dyn Stream<Item = WatchEvent> + Send>>, PulseDBError>;
}
