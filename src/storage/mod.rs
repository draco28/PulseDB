//! Storage layer abstractions for PulseDB.
//!
//! This module provides a trait-based abstraction over the storage engine,
//! allowing different backends to be used (e.g., redb, mock for testing).
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      PulseDB                                 │
//! │                         │                                    │
//! │                         ▼                                    │
//! │              ┌─────────────────────┐                        │
//! │              │   StorageEngine     │  ← Trait               │
//! │              └─────────────────────┘                        │
//! │                    ▲         ▲                              │
//! │                    │         │                              │
//! │         ┌─────────┴─┐   ┌───┴─────────┐                    │
//! │         │RedbStorage│   │ MockStorage │                    │
//! │         └───────────┘   └─────────────┘                    │
//! │           (prod)           (test)                          │
//! └─────────────────────────────────────────────────────────────┘
//! ```

pub mod redb;
pub mod schema;

pub use self::redb::RedbStorage;
pub use schema::{DatabaseMetadata, SCHEMA_VERSION};

use std::path::Path;

use crate::collective::Collective;
use crate::config::Config;
use crate::error::Result;
use crate::experience::{Experience, ExperienceUpdate};
use crate::types::{CollectiveId, ExperienceId};

/// Storage engine trait for PulseDB.
///
/// This trait defines the contract that any storage backend must implement.
/// The primary implementation is [`RedbStorage`], but other implementations
/// can be created for testing or alternative backends.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow the database to be shared
/// across threads. The engine handles internal synchronization.
///
/// # Example
///
/// ```rust,ignore
/// use pulsedb::storage::{StorageEngine, RedbStorage};
///
/// let storage = RedbStorage::open("./pulse.db", &config)?;
/// let metadata = storage.metadata();
/// println!("Schema version: {}", metadata.schema_version);
/// ```
pub trait StorageEngine: Send + Sync {
    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Returns the database metadata.
    ///
    /// The metadata includes schema version, embedding dimension, and timestamps.
    fn metadata(&self) -> &DatabaseMetadata;

    /// Closes the storage engine, flushing any pending writes.
    ///
    /// This method consumes the storage engine. After calling `close()`,
    /// the engine cannot be used.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend supports reporting flush failures.
    /// Note: the current redb backend flushes on drop (infallible), so
    /// this always returns `Ok(())` for [`RedbStorage`].
    fn close(self: Box<Self>) -> Result<()>;

    /// Returns the path to the database file, if applicable.
    ///
    /// Some storage implementations (like in-memory) may not have a path.
    fn path(&self) -> Option<&Path>;

    // =========================================================================
    // Collective Storage Operations
    // =========================================================================

    /// Saves a collective to storage.
    ///
    /// If a collective with the same ID already exists, it is overwritten.
    /// Each call opens and commits its own write transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction or serialization fails.
    fn save_collective(&self, collective: &Collective) -> Result<()>;

    /// Retrieves a collective by ID.
    ///
    /// Returns `None` if no collective with the given ID exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the read transaction or deserialization fails.
    fn get_collective(&self, id: CollectiveId) -> Result<Option<Collective>>;

    /// Lists all collectives in the database.
    ///
    /// Returns an empty vector if no collectives exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the read transaction or deserialization fails.
    fn list_collectives(&self) -> Result<Vec<Collective>>;

    /// Deletes a collective by ID.
    ///
    /// Returns `true` if the collective existed and was deleted,
    /// `false` if no collective with the given ID was found.
    ///
    /// # Errors
    ///
    /// Returns an error if the write transaction fails.
    fn delete_collective(&self, id: CollectiveId) -> Result<bool>;

    // =========================================================================
    // Experience Index Operations (for collective stats & cascade delete)
    // =========================================================================

    /// Counts experiences belonging to a collective.
    ///
    /// Queries the `experiences_by_collective` multimap index.
    /// Returns 0 if no experiences exist for the collective.
    ///
    /// # Errors
    ///
    /// Returns an error if the read transaction fails.
    fn count_experiences_in_collective(&self, id: CollectiveId) -> Result<u64>;

    /// Deletes all experiences and related index entries for a collective.
    ///
    /// Used for cascade deletion when a collective is removed. Cleans up:
    /// - Experience records
    /// - Embedding vectors
    /// - By-collective index entries
    /// - By-type index entries
    ///
    /// Returns the number of experiences deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if the write transaction fails.
    fn delete_experiences_by_collective(&self, id: CollectiveId) -> Result<u64>;

    // =========================================================================
    // Experience Storage Operations
    // =========================================================================

    /// Saves an experience and its embedding to storage.
    ///
    /// Writes atomically to 4 tables in a single transaction:
    /// - `EXPERIENCES_TABLE` — the experience record (without embedding)
    /// - `EMBEDDINGS_TABLE` — the embedding vector as raw f32 bytes
    /// - `EXPERIENCES_BY_COLLECTIVE_TABLE` — secondary index by collective+timestamp
    /// - `EXPERIENCES_BY_TYPE_TABLE` — secondary index by collective+type
    ///
    /// # Errors
    ///
    /// Returns an error if the transaction or serialization fails.
    fn save_experience(&self, experience: &Experience) -> Result<()>;

    /// Retrieves an experience by ID, including its embedding.
    ///
    /// Reads from both `EXPERIENCES_TABLE` and `EMBEDDINGS_TABLE` to
    /// reconstitute the full experience with embedding.
    ///
    /// Returns `None` if no experience with the given ID exists.
    fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>>;

    /// Updates mutable fields of an experience.
    ///
    /// Applies only the `Some` fields from the update. Immutable fields
    /// (content, embedding, collective_id, type) are not affected.
    ///
    /// Returns `true` if the experience existed and was updated,
    /// `false` if not found.
    fn update_experience(&self, id: ExperienceId, update: &ExperienceUpdate) -> Result<bool>;

    /// Permanently deletes an experience and its embedding.
    ///
    /// Removes from all 4 tables in a single transaction.
    ///
    /// Returns `true` if the experience existed and was deleted,
    /// `false` if not found.
    fn delete_experience(&self, id: ExperienceId) -> Result<bool>;

    /// Saves an embedding vector to storage.
    ///
    /// The embedding is stored as raw little-endian f32 bytes.
    fn save_embedding(&self, id: ExperienceId, embedding: &[f32]) -> Result<()>;

    /// Retrieves an embedding vector by experience ID.
    ///
    /// Returns `None` if no embedding exists for the given ID.
    fn get_embedding(&self, id: ExperienceId) -> Result<Option<Vec<f32>>>;
}

/// Opens a storage engine at the given path.
///
/// This is a convenience function that creates a [`RedbStorage`] instance.
/// For more control, use `RedbStorage::open()` directly.
///
/// # Arguments
///
/// * `path` - Path to the database file (created if it doesn't exist)
/// * `config` - Database configuration
///
/// # Errors
///
/// Returns an error if:
/// - The database file is corrupted
/// - The database is locked by another process
/// - Schema version doesn't match
/// - Embedding dimension doesn't match (for existing databases)
pub fn open_storage(path: impl AsRef<Path>, config: &Config) -> Result<Box<dyn StorageEngine>> {
    let storage = RedbStorage::open(path, config)?;
    Ok(Box::new(storage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmbeddingDimension;
    use tempfile::tempdir;

    #[test]
    fn test_open_storage() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let config = Config::default();
        let storage = open_storage(&path, &config).unwrap();

        assert_eq!(
            storage.metadata().embedding_dimension,
            EmbeddingDimension::D384
        );
        assert!(storage.path().is_some());

        storage.close().unwrap();
    }

    #[test]
    fn test_storage_engine_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RedbStorage>();
    }
}
