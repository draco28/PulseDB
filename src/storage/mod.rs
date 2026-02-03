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

use crate::config::Config;
use crate::error::Result;

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
    /// Returns an error if flushing fails.
    fn close(self: Box<Self>) -> Result<()>;

    /// Returns the path to the database file, if applicable.
    ///
    /// Some storage implementations (like in-memory) may not have a path.
    fn path(&self) -> Option<&Path>;
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
