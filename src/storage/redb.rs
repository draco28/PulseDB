//! redb storage engine implementation.
//!
//! This module provides the primary storage backend for PulseDB using
//! [redb](https://docs.rs/redb), a pure Rust embedded key-value store.
//!
//! # Features
//!
//! - ACID transactions with MVCC
//! - Single-writer, multiple-reader concurrency
//! - Automatic crash recovery
//! - Zero external dependencies (pure Rust)
//!
//! # File Layout
//!
//! When you open a database at `./pulse.db`, redb creates:
//! - `./pulse.db` - Main database file
//! - `./pulse.db.lock` - Lock file for writer coordination (may not be visible)

use std::path::{Path, PathBuf};

use ::redb::Database;
use tracing::{debug, info, instrument, warn};

use super::schema::{
    DatabaseMetadata, COLLECTIVES_TABLE, EMBEDDINGS_TABLE, EXPERIENCES_BY_COLLECTIVE_TABLE,
    EXPERIENCES_TABLE, METADATA_TABLE, SCHEMA_VERSION,
};
use super::StorageEngine;
use crate::config::{Config, EmbeddingDimension};
use crate::error::{PulseDBError, Result, StorageError, ValidationError};

/// Metadata key in the metadata table.
const METADATA_KEY: &str = "db_metadata";

/// redb storage engine wrapper.
///
/// This struct holds the redb database handle and cached metadata.
/// It implements [`StorageEngine`] for use with PulseDB.
///
/// # Thread Safety
///
/// `RedbStorage` is `Send + Sync`. redb handles internal synchronization
/// using MVCC for readers and exclusive locking for writers.
#[derive(Debug)]
pub struct RedbStorage {
    /// The redb database handle.
    db: Database,

    /// Cached database metadata.
    metadata: DatabaseMetadata,

    /// Path to the database file.
    path: PathBuf,
}

impl RedbStorage {
    /// Opens or creates a database at the given path.
    ///
    /// If the database doesn't exist, it will be created and initialized
    /// with the configuration settings. If it exists, the configuration
    /// will be validated against the stored metadata.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the database file
    /// * `config` - Database configuration
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The database file is corrupted
    /// - The database is locked by another process
    /// - Schema version doesn't match
    /// - Embedding dimension doesn't match (for existing databases)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use pulsedb::{Config, storage::RedbStorage};
    ///
    /// let storage = RedbStorage::open("./pulse.db", &Config::default())?;
    /// ```
    #[instrument(skip(config), fields(path = %path.as_ref().display()))]
    pub fn open(path: impl AsRef<Path>, config: &Config) -> Result<Self> {
        let path = path.as_ref();
        let db_exists = path.exists();

        debug!(db_exists = db_exists, "Opening storage engine");

        // Create or open the database
        let db = Self::create_database(path, config)?;

        if db_exists {
            // Validate existing database
            Self::open_existing(db, path.to_path_buf(), config)
        } else {
            // Initialize new database
            Self::initialize_new(db, path.to_path_buf(), config)
        }
    }

    /// Creates the redb database with appropriate settings.
    fn create_database(path: &Path, _config: &Config) -> Result<Database> {
        let builder = Database::builder();

        // Note: redb 2.x doesn't have set_cache_size, it manages memory internally
        // The cache_size_mb config will be used for future optimizations

        let db = builder.create(path).map_err(|e| {
            if e.to_string().contains("locked") {
                StorageError::DatabaseLocked
            } else {
                StorageError::Redb(e.to_string())
            }
        })?;

        debug!("Database file opened successfully");
        Ok(db)
    }

    /// Initializes a new database with tables and metadata.
    #[instrument(skip(db, config), fields(path = %path.display()))]
    fn initialize_new(db: Database, path: PathBuf, config: &Config) -> Result<Self> {
        info!("Initializing new database");

        let metadata = DatabaseMetadata::new(config.embedding_dimension);

        // Create all tables and write metadata in a single transaction
        let write_txn = db.begin_write().map_err(StorageError::from)?;

        {
            // Create the metadata table and write metadata
            let mut meta_table = write_txn.open_table(METADATA_TABLE)?;
            let metadata_bytes = bincode::serialize(&metadata)
                .map_err(|e| StorageError::serialization(e.to_string()))?;
            meta_table.insert(METADATA_KEY, metadata_bytes.as_slice())?;

            // Create other tables (they're created on first access)
            let _ = write_txn.open_table(COLLECTIVES_TABLE)?;
            let _ = write_txn.open_table(EXPERIENCES_TABLE)?;
            let _ = write_txn.open_table(EMBEDDINGS_TABLE)?;
            let _ = write_txn.open_multimap_table(EXPERIENCES_BY_COLLECTIVE_TABLE)?;
        }

        write_txn.commit().map_err(StorageError::from)?;

        info!(
            schema_version = SCHEMA_VERSION,
            dimension = config.embedding_dimension.size(),
            "Database initialized"
        );

        Ok(Self { db, metadata, path })
    }

    /// Opens and validates an existing database.
    #[instrument(skip(db, config), fields(path = %path.display()))]
    fn open_existing(db: Database, path: PathBuf, config: &Config) -> Result<Self> {
        info!("Opening existing database");

        // Read metadata from the database
        let read_txn = db.begin_read().map_err(StorageError::from)?;

        let metadata = {
            let meta_table = read_txn.open_table(METADATA_TABLE).map_err(|e| {
                StorageError::corrupted(format!("Cannot open metadata table: {}", e))
            })?;

            let metadata_bytes = meta_table
                .get(METADATA_KEY)
                .map_err(StorageError::from)?
                .ok_or_else(|| StorageError::corrupted("Missing database metadata"))?;

            bincode::deserialize::<DatabaseMetadata>(metadata_bytes.value())
                .map_err(|e| StorageError::corrupted(format!("Invalid metadata format: {}", e)))?
        };

        drop(read_txn);

        // Validate schema version
        if metadata.schema_version != SCHEMA_VERSION {
            warn!(
                expected = SCHEMA_VERSION,
                found = metadata.schema_version,
                "Schema version mismatch"
            );
            return Err(PulseDBError::Storage(StorageError::SchemaVersionMismatch {
                expected: SCHEMA_VERSION,
                found: metadata.schema_version,
            }));
        }

        // Validate embedding dimension
        if metadata.embedding_dimension != config.embedding_dimension {
            warn!(
                expected = config.embedding_dimension.size(),
                found = metadata.embedding_dimension.size(),
                "Embedding dimension mismatch"
            );
            return Err(PulseDBError::Validation(
                ValidationError::DimensionMismatch {
                    expected: config.embedding_dimension.size(),
                    got: metadata.embedding_dimension.size(),
                },
            ));
        }

        // Update last_opened_at timestamp
        let mut metadata = metadata;
        metadata.touch();

        let write_txn = db.begin_write().map_err(StorageError::from)?;
        {
            let mut meta_table = write_txn.open_table(METADATA_TABLE)?;
            let metadata_bytes = bincode::serialize(&metadata)
                .map_err(|e| StorageError::serialization(e.to_string()))?;
            meta_table.insert(METADATA_KEY, metadata_bytes.as_slice())?;
        }
        write_txn.commit().map_err(StorageError::from)?;

        info!(
            schema_version = metadata.schema_version,
            dimension = metadata.embedding_dimension.size(),
            "Database opened successfully"
        );

        Ok(Self { db, metadata, path })
    }

    /// Returns a reference to the underlying redb database.
    ///
    /// This is for internal use by other PulseDB modules.
    #[inline]
    #[allow(dead_code)] // Used by Collective CRUD (E1-S02) and Experience CRUD (E1-S03)
    pub(crate) fn database(&self) -> &Database {
        &self.db
    }

    /// Returns the embedding dimension configured for this database.
    #[inline]
    pub fn embedding_dimension(&self) -> EmbeddingDimension {
        self.metadata.embedding_dimension
    }
}

impl StorageEngine for RedbStorage {
    fn metadata(&self) -> &DatabaseMetadata {
        &self.metadata
    }

    #[instrument(skip(self))]
    fn close(self: Box<Self>) -> Result<()> {
        info!("Closing storage engine");

        // redb handles flushing on drop, but we explicitly drop
        // the database to ensure any errors are caught
        drop(self.db);

        info!("Storage engine closed");
        Ok(())
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}

// Implement Send and Sync - redb::Database is Send + Sync
unsafe impl Send for RedbStorage {}
unsafe impl Sync for RedbStorage {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn default_config() -> Config {
        Config::default()
    }

    #[test]
    fn test_open_creates_new_database() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        assert!(!path.exists());

        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        assert!(path.exists());
        assert_eq!(storage.metadata().schema_version, SCHEMA_VERSION);
        assert_eq!(
            storage.metadata().embedding_dimension,
            EmbeddingDimension::D384
        );

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_open_existing_database() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        // Create database
        let storage = RedbStorage::open(&path, &default_config()).unwrap();
        let created_at = storage.metadata().created_at;
        Box::new(storage).close().unwrap();

        // Reopen
        std::thread::sleep(std::time::Duration::from_millis(10));
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        // created_at should be preserved
        assert_eq!(storage.metadata().created_at, created_at);
        // last_opened_at should be updated
        assert!(storage.metadata().last_opened_at > created_at);

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_dimension_mismatch_returns_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        // Create with D384
        let config_384 = Config {
            embedding_dimension: EmbeddingDimension::D384,
            ..Default::default()
        };
        let storage = RedbStorage::open(&path, &config_384).unwrap();
        Box::new(storage).close().unwrap();

        // Try to reopen with D768
        let config_768 = Config {
            embedding_dimension: EmbeddingDimension::D768,
            ..Default::default()
        };
        let result = RedbStorage::open(&path, &config_768);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err,
            PulseDBError::Validation(ValidationError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn test_database_files_created() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pulse.db");

        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        // Main database file should exist
        assert!(path.exists());
        assert!(storage.path().is_some());
        assert_eq!(storage.path().unwrap(), path);

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_metadata_preserved_across_opens() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let config = Config {
            embedding_dimension: EmbeddingDimension::Custom(512),
            ..Default::default()
        };

        // Create
        let storage = RedbStorage::open(&path, &config).unwrap();
        assert_eq!(
            storage.metadata().embedding_dimension,
            EmbeddingDimension::Custom(512)
        );
        Box::new(storage).close().unwrap();

        // Reopen
        let storage = RedbStorage::open(&path, &config).unwrap();
        assert_eq!(
            storage.metadata().embedding_dimension,
            EmbeddingDimension::Custom(512)
        );
        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_embedding_dimension_accessor() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let config = Config {
            embedding_dimension: EmbeddingDimension::D768,
            ..Default::default()
        };

        let storage = RedbStorage::open(&path, &config).unwrap();
        assert_eq!(storage.embedding_dimension(), EmbeddingDimension::D768);

        Box::new(storage).close().unwrap();
    }
}
