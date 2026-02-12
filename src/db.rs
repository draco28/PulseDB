//! PulseDB main struct and lifecycle operations.
//!
//! The [`PulseDB`] struct is the primary interface for interacting with
//! the database. It provides methods for:
//!
//! - Opening and closing the database
//! - Managing collectives (isolation units)
//! - Recording and querying experiences
//! - Semantic search and context retrieval
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use pulsedb::{PulseDB, Config};
//!
//! // Open or create a database
//! let db = PulseDB::open("./pulse.db", Config::default())?;
//!
//! // Create a collective for your project
//! let collective = db.create_collective("my-project")?;
//!
//! // Record an experience
//! db.record_experience(NewExperience {
//!     collective_id: collective,
//!     content: "Always validate user input".to_string(),
//!     experience_type: ExperienceType::Lesson,
//!     ..Default::default()
//! })?;
//!
//! // Close when done
//! db.close()?;
//! ```
//!
//! # Thread Safety
//!
//! `PulseDB` is `Send + Sync` and can be shared across threads using `Arc`.
//! The underlying storage uses MVCC for concurrent reads with exclusive
//! write locking.
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use pulsedb::PulseDB;
//!
//! let db = Arc::new(PulseDB::open("./pulse.db", Config::default())?);
//!
//! // Clone Arc for use in another thread
//! let db_clone = Arc::clone(&db);
//! std::thread::spawn(move || {
//!     // Safe to use db_clone here
//! });
//! ```

use std::path::Path;

use tracing::{info, instrument};

use crate::config::Config;
use crate::embedding::{create_embedding_service, EmbeddingService};
use crate::error::{PulseDBError, Result};
use crate::storage::{open_storage, DatabaseMetadata, StorageEngine};

/// The main PulseDB database handle.
///
/// This is the primary interface for all database operations. Create an
/// instance with [`PulseDB::open()`] and close it with [`PulseDB::close()`].
///
/// # Ownership
///
/// `PulseDB` owns its storage and embedding service. When you call `close()`,
/// the database is consumed and cannot be used afterward. This ensures
/// resources are properly released.
pub struct PulseDB {
    /// Storage engine (redb or mock for testing).
    storage: Box<dyn StorageEngine>,

    /// Embedding service (external or ONNX).
    /// Used by experience recording (E1-S03) and search (Phase 2).
    #[allow(dead_code)]
    embedding: Box<dyn EmbeddingService>,

    /// Configuration used to open this database.
    config: Config,
}

impl std::fmt::Debug for PulseDB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PulseDB")
            .field("config", &self.config)
            .field("embedding_dimension", &self.embedding_dimension())
            .finish_non_exhaustive()
    }
}

impl PulseDB {
    /// Opens or creates a PulseDB database at the specified path.
    ///
    /// If the database doesn't exist, it will be created with the given
    /// configuration. If it exists, the configuration will be validated
    /// against the stored settings (e.g., embedding dimension must match).
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the database file (created if it doesn't exist)
    /// * `config` - Configuration options for the database
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Configuration is invalid (see [`Config::validate`])
    /// - Database file is corrupted
    /// - Database is locked by another process
    /// - Schema version doesn't match (needs migration)
    /// - Embedding dimension doesn't match existing database
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use pulsedb::{PulseDB, Config, EmbeddingDimension};
    ///
    /// // Open with default configuration
    /// let db = PulseDB::open("./pulse.db", Config::default())?;
    ///
    /// // Open with custom embedding dimension
    /// let db = PulseDB::open("./pulse.db", Config {
    ///     embedding_dimension: EmbeddingDimension::D768,
    ///     ..Default::default()
    /// })?;
    /// ```
    #[instrument(skip(config), fields(path = %path.as_ref().display()))]
    pub fn open(path: impl AsRef<Path>, config: Config) -> Result<Self> {
        // Validate configuration first
        config.validate().map_err(PulseDBError::from)?;

        info!("Opening PulseDB");

        // Open storage engine
        let storage = open_storage(&path, &config)?;

        // Create embedding service
        let embedding = create_embedding_service(&config)?;

        info!(
            dimension = config.embedding_dimension.size(),
            sync_mode = ?config.sync_mode,
            "PulseDB opened successfully"
        );

        Ok(Self {
            storage,
            embedding,
            config,
        })
    }

    /// Closes the database, flushing all pending writes.
    ///
    /// This method consumes the `PulseDB` instance, ensuring it cannot
    /// be used after closing. The underlying storage engine flushes all
    /// buffered data to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage backend reports a flush failure.
    /// Note: the current redb backend flushes durably on drop, so this
    /// always returns `Ok(())` in practice.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use pulsedb::{PulseDB, Config};
    ///
    /// let db = PulseDB::open("./pulse.db", Config::default())?;
    /// // ... use the database ...
    /// db.close()?;  // db is consumed here
    /// // db.something() // Compile error: db was moved
    /// ```
    #[instrument(skip(self))]
    pub fn close(self) -> Result<()> {
        info!("Closing PulseDB");

        // Close storage (flushes pending writes)
        self.storage.close()?;

        info!("PulseDB closed successfully");
        Ok(())
    }

    /// Returns a reference to the database configuration.
    ///
    /// This is the configuration that was used to open the database.
    /// Note that some settings (like embedding dimension) are locked
    /// on database creation and cannot be changed.
    #[inline]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the database metadata.
    ///
    /// Metadata includes schema version, embedding dimension, and timestamps
    /// for when the database was created and last opened.
    #[inline]
    pub fn metadata(&self) -> &DatabaseMetadata {
        self.storage.metadata()
    }

    /// Returns the embedding dimension configured for this database.
    ///
    /// All embeddings stored in this database must have exactly this
    /// many dimensions.
    #[inline]
    pub fn embedding_dimension(&self) -> usize {
        self.config.embedding_dimension.size()
    }

    // =========================================================================
    // Internal Accessors (for use by feature modules)
    // =========================================================================

    /// Returns a reference to the storage engine.
    ///
    /// This is for internal use by other PulseDB modules.
    #[inline]
    #[allow(dead_code)] // Used by Collective CRUD (E1-S02) and Experience CRUD (E1-S03)
    pub(crate) fn storage(&self) -> &dyn StorageEngine {
        self.storage.as_ref()
    }

    /// Returns a reference to the embedding service.
    ///
    /// This is for internal use by other PulseDB modules.
    #[inline]
    #[allow(dead_code)] // Used by Experience CRUD (E1-S03)
    pub(crate) fn embedding(&self) -> &dyn EmbeddingService {
        self.embedding.as_ref()
    }

    // =========================================================================
    // Placeholder methods for future tickets
    // =========================================================================

    // E1-S02: Collective CRUD
    // pub fn create_collective(&self, name: &str) -> Result<CollectiveId>;
    // pub fn get_collective(&self, id: CollectiveId) -> Result<Option<Collective>>;
    // pub fn list_collectives(&self) -> Result<Vec<Collective>>;
    // pub fn delete_collective(&self, id: CollectiveId) -> Result<()>;

    // E1-S03: Experience CRUD
    // pub fn record_experience(&self, exp: NewExperience) -> Result<ExperienceId>;
    // pub fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>>;
    // pub fn update_experience(&self, id: ExperienceId, update: ExperienceUpdate) -> Result<()>;
    // pub fn archive_experience(&self, id: ExperienceId) -> Result<()>;
    // pub fn delete_experience(&self, id: ExperienceId) -> Result<()>;
}

// PulseDB is auto Send + Sync: Box<dyn StorageEngine + Send + Sync>,
// Box<dyn EmbeddingService + Send + Sync>, and Config are all Send + Sync.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmbeddingDimension;
    use tempfile::tempdir;

    #[test]
    fn test_open_creates_database() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let db = PulseDB::open(&path, Config::default()).unwrap();

        assert!(path.exists());
        assert_eq!(db.embedding_dimension(), 384);

        db.close().unwrap();
    }

    #[test]
    fn test_open_existing_database() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        // Create
        let db = PulseDB::open(&path, Config::default()).unwrap();
        db.close().unwrap();

        // Reopen
        let db = PulseDB::open(&path, Config::default()).unwrap();
        assert_eq!(db.embedding_dimension(), 384);
        db.close().unwrap();
    }

    #[test]
    fn test_config_validation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let invalid_config = Config {
            cache_size_mb: 0, // Invalid
            ..Default::default()
        };

        let result = PulseDB::open(&path, invalid_config);
        assert!(result.is_err());
    }

    #[test]
    fn test_dimension_mismatch() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        // Create with D384
        let db = PulseDB::open(
            &path,
            Config {
                embedding_dimension: EmbeddingDimension::D384,
                ..Default::default()
            },
        )
        .unwrap();
        db.close().unwrap();

        // Try to reopen with D768
        let result = PulseDB::open(
            &path,
            Config {
                embedding_dimension: EmbeddingDimension::D768,
                ..Default::default()
            },
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_metadata_access() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let db = PulseDB::open(&path, Config::default()).unwrap();

        let metadata = db.metadata();
        assert_eq!(metadata.embedding_dimension, EmbeddingDimension::D384);

        db.close().unwrap();
    }

    #[test]
    fn test_pulsedb_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PulseDB>();
    }
}
