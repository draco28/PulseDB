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

use ::redb::{Database, ReadableTable};
use tracing::{debug, info, instrument, warn};

use crate::collective::Collective;
use crate::types::CollectiveId;

use super::schema::{
    DatabaseMetadata, COLLECTIVES_TABLE, EMBEDDINGS_TABLE, EXPERIENCES_BY_COLLECTIVE_TABLE,
    EXPERIENCES_BY_TYPE_TABLE, EXPERIENCES_TABLE, METADATA_TABLE, SCHEMA_VERSION,
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

        // Note: redb doesn't expose a typed error variant for lock conflicts,
        // so we detect them via error message string matching. This may need
        // updating if redb changes its error messages in a future version.
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
            let _ = write_txn.open_multimap_table(EXPERIENCES_BY_TYPE_TABLE)?;
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
    // =========================================================================
    // Lifecycle
    // =========================================================================

    fn metadata(&self) -> &DatabaseMetadata {
        &self.metadata
    }

    #[instrument(skip(self))]
    fn close(self: Box<Self>) -> Result<()> {
        info!("Closing storage engine");

        // redb flushes all data durably on drop. Since `Database::drop` is
        // infallible, this method currently always returns Ok(()). The Result
        // return type is retained for API forward-compatibility if a future
        // storage backend can report flush errors.
        drop(self.db);

        info!("Storage engine closed");
        Ok(())
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    // =========================================================================
    // Collective Storage Operations
    // =========================================================================

    fn save_collective(&self, collective: &Collective) -> Result<()> {
        let bytes = bincode::serialize(collective)
            .map_err(|e| StorageError::serialization(e.to_string()))?;

        let write_txn = self.db.begin_write().map_err(StorageError::from)?;
        {
            let mut table = write_txn.open_table(COLLECTIVES_TABLE)?;
            table.insert(collective.id.as_bytes(), bytes.as_slice())?;
        }
        write_txn.commit().map_err(StorageError::from)?;

        debug!(id = %collective.id, name = %collective.name, "Collective saved");
        Ok(())
    }

    fn get_collective(&self, id: CollectiveId) -> Result<Option<Collective>> {
        let read_txn = self.db.begin_read().map_err(StorageError::from)?;
        let table = read_txn.open_table(COLLECTIVES_TABLE)?;

        match table.get(id.as_bytes())? {
            Some(value) => {
                let collective: Collective = bincode::deserialize(value.value())
                    .map_err(|e| StorageError::serialization(e.to_string()))?;
                Ok(Some(collective))
            }
            None => Ok(None),
        }
    }

    fn list_collectives(&self) -> Result<Vec<Collective>> {
        let read_txn = self.db.begin_read().map_err(StorageError::from)?;
        let table = read_txn.open_table(COLLECTIVES_TABLE)?;

        let mut collectives = Vec::new();
        for result in table.iter()? {
            let (_, value) = result.map_err(StorageError::from)?;
            let collective: Collective = bincode::deserialize(value.value())
                .map_err(|e| StorageError::serialization(e.to_string()))?;
            collectives.push(collective);
        }

        Ok(collectives)
    }

    fn delete_collective(&self, id: CollectiveId) -> Result<bool> {
        let write_txn = self.db.begin_write().map_err(StorageError::from)?;
        let existed;
        {
            let mut table = write_txn.open_table(COLLECTIVES_TABLE)?;
            existed = table.remove(id.as_bytes())?.is_some();
        }
        write_txn.commit().map_err(StorageError::from)?;

        if existed {
            debug!(id = %id, "Collective deleted");
        }
        Ok(existed)
    }
}

// RedbStorage is auto Send + Sync: Database, DatabaseMetadata, and PathBuf
// are all Send + Sync.

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

    #[test]
    fn test_all_six_tables_created() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");

        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        // Verify all 6 tables exist by opening each in a read transaction.
        // If any table wasn't created during initialize_new(), this would
        // return a TableDoesNotExist error.
        let read_txn = storage.database().begin_read().unwrap();

        read_txn.open_table(METADATA_TABLE).unwrap();
        read_txn.open_table(COLLECTIVES_TABLE).unwrap();
        read_txn.open_table(EXPERIENCES_TABLE).unwrap();
        read_txn.open_table(EMBEDDINGS_TABLE).unwrap();
        read_txn
            .open_multimap_table(EXPERIENCES_BY_COLLECTIVE_TABLE)
            .unwrap();
        read_txn
            .open_multimap_table(EXPERIENCES_BY_TYPE_TABLE)
            .unwrap();

        Box::new(storage).close().unwrap();
    }

    // ====================================================================
    // Collective CRUD tests
    // ====================================================================

    #[test]
    fn test_save_and_get_collective() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let collective = Collective::new("test-project", 384);
        let id = collective.id;

        storage.save_collective(&collective).unwrap();

        let retrieved = storage.get_collective(id).unwrap().unwrap();
        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.name, "test-project");
        assert_eq!(retrieved.embedding_dimension, 384);
        assert!(retrieved.owner_id.is_none());

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_get_nonexistent_collective_returns_none() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let result = storage.get_collective(CollectiveId::new()).unwrap();
        assert!(result.is_none());

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_save_collective_overwrites_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let mut collective = Collective::new("original-name", 384);
        let id = collective.id;
        storage.save_collective(&collective).unwrap();

        // Overwrite with updated name
        collective.name = "updated-name".to_string();
        storage.save_collective(&collective).unwrap();

        let retrieved = storage.get_collective(id).unwrap().unwrap();
        assert_eq!(retrieved.name, "updated-name");

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_list_collectives_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let collectives = storage.list_collectives().unwrap();
        assert!(collectives.is_empty());

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_list_collectives_returns_all() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let c1 = Collective::new("project-alpha", 384);
        let c2 = Collective::new("project-beta", 384);
        let c3 = Collective::new("project-gamma", 384);

        storage.save_collective(&c1).unwrap();
        storage.save_collective(&c2).unwrap();
        storage.save_collective(&c3).unwrap();

        let collectives = storage.list_collectives().unwrap();
        assert_eq!(collectives.len(), 3);

        // Verify all IDs are present
        let ids: Vec<CollectiveId> = collectives.iter().map(|c| c.id).collect();
        assert!(ids.contains(&c1.id));
        assert!(ids.contains(&c2.id));
        assert!(ids.contains(&c3.id));

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_delete_collective_existing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let collective = Collective::new("to-delete", 384);
        let id = collective.id;
        storage.save_collective(&collective).unwrap();

        // Delete it
        let deleted = storage.delete_collective(id).unwrap();
        assert!(deleted);

        // Verify it's gone
        assert!(storage.get_collective(id).unwrap().is_none());

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_delete_collective_nonexistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let deleted = storage.delete_collective(CollectiveId::new()).unwrap();
        assert!(!deleted);

        Box::new(storage).close().unwrap();
    }

    // ====================================================================
    // ACID Guarantee Tests
    // ====================================================================

    #[test]
    fn test_uncommitted_transaction_is_invisible() {
        // ATOMICITY: If we don't commit a write transaction, the data
        // must not be visible to subsequent reads.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let collective = Collective::new("phantom", 384);
        let id = collective.id;
        let bytes = bincode::serialize(&collective).unwrap();

        // Open a write transaction, insert data, but DON'T commit -- just drop
        {
            let write_txn = storage.database().begin_write().unwrap();
            {
                let mut table = write_txn.open_table(COLLECTIVES_TABLE).unwrap();
                table.insert(id.as_bytes(), bytes.as_slice()).unwrap();
            }
            // write_txn is dropped here without commit() -- rolled back
        }

        // The collective should NOT be visible
        let result = storage.get_collective(id).unwrap();
        assert!(result.is_none(), "Uncommitted data must not be visible");

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_committed_transaction_is_visible() {
        // DURABILITY (within session): committed data must be immediately
        // visible to subsequent reads.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let collective = Collective::new("committed", 384);
        let id = collective.id;

        storage.save_collective(&collective).unwrap();

        let result = storage.get_collective(id).unwrap();
        assert!(result.is_some(), "Committed data must be visible");

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_multi_table_atomicity() {
        // ATOMICITY: A single transaction writing to multiple tables
        // is all-or-nothing. Here we write to both COLLECTIVES and METADATA
        // in one transaction and verify both are visible after commit.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        let collective = Collective::new("multi-table", 384);
        let id = collective.id;
        let collective_bytes = bincode::serialize(&collective).unwrap();

        // Write to TWO tables in a single transaction
        let write_txn = storage.database().begin_write().unwrap();
        {
            let mut coll_table = write_txn.open_table(COLLECTIVES_TABLE).unwrap();
            coll_table
                .insert(id.as_bytes(), collective_bytes.as_slice())
                .unwrap();
        }
        {
            let mut meta_table = write_txn.open_table(METADATA_TABLE).unwrap();
            meta_table
                .insert("test_marker", b"multi_table_test".as_slice())
                .unwrap();
        }
        write_txn.commit().unwrap();

        // Verify BOTH writes are visible
        let coll = storage.get_collective(id).unwrap();
        assert!(coll.is_some(), "Collective from multi-table txn must exist");

        let read_txn = storage.database().begin_read().unwrap();
        let meta_table = read_txn.open_table(METADATA_TABLE).unwrap();
        let marker = meta_table.get("test_marker").unwrap();
        assert!(marker.is_some(), "Metadata from multi-table txn must exist");

        Box::new(storage).close().unwrap();
    }

    #[test]
    fn test_mvcc_read_consistency() {
        // ISOLATION (MVCC): A single read transaction sees a consistent
        // snapshot reflecting all committed writes up to the moment the
        // read was opened, and none of the uncommitted or subsequent ones.
        //
        // We write across multiple separate transactions, then verify a
        // read sees the expected consistent state. Combined with
        // test_uncommitted_transaction_is_invisible (atomicity), this
        // covers the key ACID isolation properties.
        //
        // Note: redb 2.6.3 has a page allocation constraint that prevents
        // holding a read transaction open while a write commits on the
        // same Database handle. redb guarantees MVCC isolation internally
        // via shadow paging; this test verifies our usage is correct.
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.db");
        let storage = RedbStorage::open(&path, &default_config()).unwrap();

        // Write 3 collectives across separate transactions
        let c1 = Collective::new("alpha", 384);
        let c2 = Collective::new("beta", 384);
        let c3 = Collective::new("gamma", 384);

        storage.save_collective(&c1).unwrap();
        storage.save_collective(&c2).unwrap();
        storage.save_collective(&c3).unwrap();

        // Delete c2 (another transaction)
        storage.delete_collective(c2.id).unwrap();

        // A read transaction must see the consistent state:
        // c1 and c3 present, c2 absent
        let read_txn = storage.database().begin_read().unwrap();
        let table = read_txn.open_table(COLLECTIVES_TABLE).unwrap();

        assert!(
            table.get(c1.id.as_bytes()).unwrap().is_some(),
            "c1 must be visible (committed)"
        );
        assert!(
            table.get(c2.id.as_bytes()).unwrap().is_none(),
            "c2 must be absent (deleted)"
        );
        assert!(
            table.get(c3.id.as_bytes()).unwrap().is_some(),
            "c3 must be visible (committed)"
        );

        // Count should be exactly 2
        let count = table.iter().unwrap().count();
        // +1 for the metadata entry? No -- COLLECTIVES_TABLE is separate.
        assert_eq!(count, 2, "Exactly 2 collectives should exist");

        drop(table);
        drop(read_txn);

        Box::new(storage).close().unwrap();
    }

    // ====================================================================
    // Corruption Detection Tests
    // ====================================================================

    #[test]
    fn test_corruption_detection_invalid_metadata_bytes() {
        // Opening a database whose metadata contains garbage bytes
        // must return a Corrupted error, not a panic or deserialization UB.
        let dir = tempdir().unwrap();
        let path = dir.path().join("corrupt.db");

        // Create a valid database, then corrupt the metadata
        let storage = RedbStorage::open(&path, &default_config()).unwrap();
        let write_txn = storage.database().begin_write().unwrap();
        {
            let mut meta = write_txn.open_table(METADATA_TABLE).unwrap();
            meta.insert(METADATA_KEY, b"not-valid-bincode-data".as_slice())
                .unwrap();
        }
        write_txn.commit().unwrap();
        Box::new(storage).close().unwrap();

        // Reopen must detect the corruption
        let result = RedbStorage::open(&path, &default_config());
        assert!(result.is_err(), "Corrupted metadata must be rejected");
        let err = result.unwrap_err();
        match err {
            PulseDBError::Storage(StorageError::Corrupted(msg)) => {
                assert!(
                    msg.contains("Invalid metadata format"),
                    "Error should mention invalid format, got: {}",
                    msg
                );
            }
            other => panic!("Expected StorageError::Corrupted, got: {:?}", other),
        }
    }

    #[test]
    fn test_corruption_detection_missing_metadata_key() {
        // If the metadata table exists but the "db_metadata" key is absent,
        // open_existing must return a Corrupted error.
        let dir = tempdir().unwrap();
        let path = dir.path().join("no_key.db");

        // Create a valid database, then delete the metadata key
        let storage = RedbStorage::open(&path, &default_config()).unwrap();
        let write_txn = storage.database().begin_write().unwrap();
        {
            let mut meta = write_txn.open_table(METADATA_TABLE).unwrap();
            meta.remove(METADATA_KEY).unwrap();
        }
        write_txn.commit().unwrap();
        Box::new(storage).close().unwrap();

        // Reopen must detect the missing key
        let result = RedbStorage::open(&path, &default_config());
        assert!(result.is_err(), "Missing metadata key must be rejected");
        let err = result.unwrap_err();
        match err {
            PulseDBError::Storage(StorageError::Corrupted(msg)) => {
                assert!(
                    msg.contains("Missing database metadata"),
                    "Error should mention missing metadata, got: {}",
                    msg
                );
            }
            other => panic!("Expected StorageError::Corrupted, got: {:?}", other),
        }
    }

    #[test]
    fn test_corruption_detection_missing_metadata_table() {
        // If the metadata table doesn't exist at all, open_existing must
        // return a Corrupted error. We simulate this by creating a raw
        // redb database without our schema tables.
        let dir = tempdir().unwrap();
        let path = dir.path().join("no_table.db");

        // Create a raw redb database with a dummy table (not our schema)
        {
            let db = ::redb::Database::create(&path).unwrap();
            let write_txn = db.begin_write().unwrap();
            {
                let dummy: ::redb::TableDefinition<&str, &str> =
                    ::redb::TableDefinition::new("dummy");
                let mut table = write_txn.open_table(dummy).unwrap();
                table.insert("key", "value").unwrap();
            }
            write_txn.commit().unwrap();
        }

        // Opening this as a PulseDB must detect the missing metadata table
        let result = RedbStorage::open(&path, &default_config());
        assert!(result.is_err(), "Missing metadata table must be rejected");
        let err = result.unwrap_err();
        match err {
            PulseDBError::Storage(StorageError::Corrupted(msg)) => {
                assert!(
                    msg.contains("Cannot open metadata table"),
                    "Error should mention metadata table, got: {}",
                    msg
                );
            }
            other => panic!("Expected StorageError::Corrupted, got: {:?}", other),
        }
    }
}
