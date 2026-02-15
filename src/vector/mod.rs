//! Vector index abstractions for semantic search.
//!
//! This module provides a trait-based abstraction over vector indexes,
//! allowing different ANN (Approximate Nearest Neighbor) backends.
//! The primary implementation uses [`hnsw_rs`] (pure Rust, ADR-005).
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────┐
//! │         VectorIndex trait         │
//! └──────────┬───────────────────────┘
//!            │
//!    ┌───────┴────────┐
//!    │   HnswIndex    │  (hnsw_rs wrapper)
//!    └────────────────┘
//! ```
//!
//! Embeddings stored in redb are the **source of truth**. The HNSW index
//! is a derived, rebuildable structure — if files are missing or corrupt,
//! rebuild from stored embeddings.

mod hnsw;

pub use hnsw::HnswIndex;

use std::path::Path;

use crate::error::Result;

/// Vector index trait for approximate nearest neighbor search.
///
/// Implementations must be `Send + Sync` for use inside `PulseDB`.
/// IDs are `usize` to align with hnsw_rs's `DataId` type.
///
/// All mutating methods (`insert`, `delete`) take `&self` and use
/// interior mutability. This enables concurrent reads during search
/// while writes are serialized internally.
pub trait VectorIndex: Send + Sync {
    /// Inserts a single vector with the given ID.
    fn insert(&self, id: usize, embedding: &[f32]) -> Result<()>;

    /// Inserts a batch of vectors.
    ///
    /// More efficient than individual inserts for large batches
    /// due to reduced locking overhead and parallel insertion.
    fn insert_batch(&self, items: &[(&Vec<f32>, usize)]) -> Result<()>;

    /// Searches for the k nearest neighbors to the query vector.
    ///
    /// Returns `(id, distance)` pairs sorted by distance ascending
    /// (closest first). Distance metric is cosine distance:
    /// 0.0 = identical, 2.0 = opposite.
    fn search(&self, query: &[f32], k: usize, ef_search: usize) -> Result<Vec<(usize, f32)>>;

    /// Searches with a filter predicate applied during traversal.
    ///
    /// Only points where `filter(id)` returns `true` are considered.
    /// This is filter-during-traversal, NOT post-filtering — critical
    /// for maintaining result count when many points are filtered.
    ///
    /// The filter must implement `hnsw_rs::FilterT` (closures do
    /// automatically via blanket impl).
    fn search_filtered(
        &self,
        query: &[f32],
        k: usize,
        ef_search: usize,
        filter: &(dyn Fn(&usize) -> bool + Sync),
    ) -> Result<Vec<(usize, f32)>>;

    /// Marks an ID as deleted (soft-delete).
    ///
    /// The vector remains in the graph but is excluded from search
    /// results. HNSW graphs don't support point removal — removing
    /// nodes breaks proximity edges that other nodes rely on.
    fn delete(&self, id: usize) -> Result<()>;

    /// Returns true if the given ID is marked as deleted.
    fn is_deleted(&self, id: usize) -> bool;

    /// Returns the number of active (non-deleted) vectors.
    fn len(&self) -> usize;

    /// Returns true if the index has no active vectors.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Persists index metadata to disk.
    fn save(&self, dir: &Path, name: &str) -> Result<()>;
}
