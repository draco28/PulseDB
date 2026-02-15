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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use tracing::{info, instrument, warn};

use crate::collective::types::CollectiveStats;
use crate::collective::{validate_collective_name, Collective};
use crate::config::{Config, EmbeddingProvider};
use crate::embedding::{create_embedding_service, EmbeddingService};
use crate::error::{NotFoundError, PulseDBError, Result};
use crate::experience::{
    validate_experience_update, validate_new_experience, Experience, ExperienceUpdate,
    NewExperience,
};
use crate::storage::{open_storage, DatabaseMetadata, StorageEngine};
use crate::types::{CollectiveId, ExperienceId, Timestamp};
use crate::vector::HnswIndex;

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
    embedding: Box<dyn EmbeddingService>,

    /// Configuration used to open this database.
    config: Config,

    /// Per-collective HNSW vector indexes for semantic search.
    ///
    /// Outer RwLock protects the HashMap (add/remove collectives).
    /// Each HnswIndex has its own internal RwLock for concurrent search+insert.
    vectors: RwLock<HashMap<CollectiveId, HnswIndex>>,
}

impl std::fmt::Debug for PulseDB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let vector_count = self
            .vectors
            .read()
            .map(|v| v.len())
            .unwrap_or(0);
        f.debug_struct("PulseDB")
            .field("config", &self.config)
            .field("embedding_dimension", &self.embedding_dimension())
            .field("vector_indexes", &vector_count)
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

        // Load or rebuild HNSW indexes for all existing collectives
        let vectors = Self::load_all_indexes(&*storage, &config)?;

        info!(
            dimension = config.embedding_dimension.size(),
            sync_mode = ?config.sync_mode,
            collectives = vectors.len(),
            "PulseDB opened successfully"
        );

        Ok(Self {
            storage,
            embedding,
            config,
            vectors: RwLock::new(vectors),
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

        // Persist HNSW indexes BEFORE closing storage.
        // If HNSW save fails, storage is still open for potential recovery.
        // On next open(), stale/missing HNSW files trigger a rebuild from redb.
        if let Some(hnsw_dir) = self.hnsw_dir() {
            let vectors = self
                .vectors
                .read()
                .map_err(|_| PulseDBError::vector("Vectors lock poisoned during close"))?;
            for (collective_id, index) in vectors.iter() {
                if let Err(e) = index.save_to_dir(&hnsw_dir, &collective_id.to_string()) {
                    warn!(
                        collective = %collective_id,
                        error = %e,
                        "Failed to save HNSW index (will rebuild on next open)"
                    );
                }
            }
        }

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
    #[allow(dead_code)] // Will be used by search (Phase 2) and other modules
    pub(crate) fn storage(&self) -> &dyn StorageEngine {
        self.storage.as_ref()
    }

    /// Returns a reference to the embedding service.
    ///
    /// This is for internal use by other PulseDB modules.
    #[inline]
    #[allow(dead_code)] // Will be used by search (Phase 2) and other modules
    pub(crate) fn embedding(&self) -> &dyn EmbeddingService {
        self.embedding.as_ref()
    }

    // =========================================================================
    // HNSW Index Lifecycle
    // =========================================================================

    /// Returns the directory for HNSW index files.
    ///
    /// Derives `{db_path}.hnsw/` from the storage path. Returns `None` if
    /// the storage has no file path (e.g., in-memory tests).
    fn hnsw_dir(&self) -> Option<PathBuf> {
        self.storage.path().map(|p| {
            let mut hnsw_path = p.as_os_str().to_owned();
            hnsw_path.push(".hnsw");
            PathBuf::from(hnsw_path)
        })
    }

    /// Loads or rebuilds HNSW indexes for all existing collectives.
    ///
    /// For each collective in storage:
    /// 1. Try loading metadata from `.hnsw.meta` file
    /// 2. Rebuild the graph from redb embeddings (always, since we can't
    ///    load the graph due to hnsw_rs lifetime constraints)
    /// 3. Restore deleted set from metadata if available
    fn load_all_indexes(
        storage: &dyn StorageEngine,
        config: &Config,
    ) -> Result<HashMap<CollectiveId, HnswIndex>> {
        let collectives = storage.list_collectives()?;
        let mut vectors = HashMap::with_capacity(collectives.len());

        let hnsw_dir = storage.path().map(|p| {
            let mut hnsw_path = p.as_os_str().to_owned();
            hnsw_path.push(".hnsw");
            PathBuf::from(hnsw_path)
        });

        for collective in &collectives {
            let dimension = collective.embedding_dimension as usize;

            // List all experience IDs in this collective
            let exp_ids = storage.list_experience_ids_in_collective(collective.id)?;

            // Load embeddings from redb (source of truth)
            let mut embeddings = Vec::with_capacity(exp_ids.len());
            for exp_id in &exp_ids {
                if let Some(embedding) = storage.get_embedding(*exp_id)? {
                    embeddings.push((*exp_id, embedding));
                }
            }

            // Try loading metadata (for deleted set and ID mappings)
            let metadata = hnsw_dir
                .as_ref()
                .and_then(|dir| HnswIndex::load_metadata(dir, &collective.id.to_string()).ok())
                .flatten();

            // Rebuild the HNSW graph from embeddings
            let index = if embeddings.is_empty() {
                HnswIndex::new(dimension, &config.hnsw)
            } else {
                let start = std::time::Instant::now();
                let idx =
                    HnswIndex::rebuild_from_embeddings(dimension, &config.hnsw, embeddings)?;
                info!(
                    collective = %collective.id,
                    vectors = idx.active_count(),
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "Rebuilt HNSW index from redb embeddings"
                );
                idx
            };

            // Restore deleted set from metadata if available
            if let Some(meta) = metadata {
                index.restore_deleted_set(&meta.deleted)?;
            }

            vectors.insert(collective.id, index);
        }

        Ok(vectors)
    }

    /// Executes a closure with the HNSW index for a collective.
    ///
    /// This is the primary accessor for vector search operations (used by
    /// `search_similar()`). The closure runs while the outer RwLock guard
    /// is held (read lock), so the HnswIndex reference stays valid.
    /// Returns `None` if no index exists for the collective.
    #[doc(hidden)]
    pub fn with_vector_index<F, R>(
        &self,
        collective_id: CollectiveId,
        f: F,
    ) -> Result<Option<R>>
    where
        F: FnOnce(&HnswIndex) -> Result<R>,
    {
        let vectors = self
            .vectors
            .read()
            .map_err(|_| PulseDBError::vector("Vectors lock poisoned"))?;
        match vectors.get(&collective_id) {
            Some(index) => Ok(Some(f(index)?)),
            None => Ok(None),
        }
    }

    // =========================================================================
    // Test Helpers
    // =========================================================================

    /// Returns a reference to the storage engine for integration testing.
    ///
    /// This method is intentionally hidden from documentation. It provides
    /// test-only access to the storage layer for verifying ACID guarantees
    /// and crash recovery. Production code should use the public PulseDB API.
    #[doc(hidden)]
    #[inline]
    pub fn storage_for_test(&self) -> &dyn StorageEngine {
        self.storage.as_ref()
    }

    // =========================================================================
    // Collective Management (E1-S02)
    // =========================================================================

    /// Creates a new collective with the given name.
    ///
    /// The collective's embedding dimension is locked to the database's
    /// configured dimension at creation time.
    ///
    /// # Arguments
    ///
    /// * `name` - Human-readable name (1-255 characters, not whitespace-only)
    ///
    /// # Errors
    ///
    /// Returns a validation error if the name is empty, whitespace-only,
    /// or exceeds 255 characters.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let id = db.create_collective("my-project")?;
    /// ```
    #[instrument(skip(self))]
    pub fn create_collective(&self, name: &str) -> Result<CollectiveId> {
        validate_collective_name(name)?;

        let dimension = self.config.embedding_dimension.size() as u16;
        let collective = Collective::new(name, dimension);
        let id = collective.id;

        // Persist to redb first (source of truth)
        self.storage.save_collective(&collective)?;

        // Create empty HNSW index for this collective
        let index = HnswIndex::new(dimension as usize, &self.config.hnsw);
        self.vectors
            .write()
            .map_err(|_| PulseDBError::vector("Vectors lock poisoned"))?
            .insert(id, index);

        info!(id = %id, name = %name, "Collective created");
        Ok(id)
    }

    /// Creates a new collective with an owner for multi-tenancy.
    ///
    /// Same as [`create_collective`](Self::create_collective) but assigns
    /// an owner ID, enabling filtering with
    /// [`list_collectives_by_owner`](Self::list_collectives_by_owner).
    ///
    /// # Arguments
    ///
    /// * `name` - Human-readable name (1-255 characters)
    /// * `owner_id` - Owner identifier (must not be empty)
    ///
    /// # Errors
    ///
    /// Returns a validation error if the name or owner_id is invalid.
    #[instrument(skip(self))]
    pub fn create_collective_with_owner(&self, name: &str, owner_id: &str) -> Result<CollectiveId> {
        validate_collective_name(name)?;

        if owner_id.is_empty() {
            return Err(PulseDBError::from(
                crate::error::ValidationError::required_field("owner_id"),
            ));
        }

        let dimension = self.config.embedding_dimension.size() as u16;
        let collective = Collective::with_owner(name, owner_id, dimension);
        let id = collective.id;

        // Persist to redb first (source of truth)
        self.storage.save_collective(&collective)?;

        // Create empty HNSW index for this collective
        let index = HnswIndex::new(dimension as usize, &self.config.hnsw);
        self.vectors
            .write()
            .map_err(|_| PulseDBError::vector("Vectors lock poisoned"))?
            .insert(id, index);

        info!(id = %id, name = %name, owner = %owner_id, "Collective created with owner");
        Ok(id)
    }

    /// Returns a collective by ID, or `None` if not found.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if let Some(collective) = db.get_collective(id)? {
    ///     println!("Found: {}", collective.name);
    /// }
    /// ```
    #[instrument(skip(self))]
    pub fn get_collective(&self, id: CollectiveId) -> Result<Option<Collective>> {
        self.storage.get_collective(id)
    }

    /// Lists all collectives in the database.
    ///
    /// Returns an empty vector if no collectives exist.
    pub fn list_collectives(&self) -> Result<Vec<Collective>> {
        self.storage.list_collectives()
    }

    /// Lists collectives filtered by owner ID.
    ///
    /// Returns only collectives whose `owner_id` matches the given value.
    /// Returns an empty vector if no matching collectives exist.
    pub fn list_collectives_by_owner(&self, owner_id: &str) -> Result<Vec<Collective>> {
        let all = self.storage.list_collectives()?;
        Ok(all
            .into_iter()
            .filter(|c| c.owner_id.as_deref() == Some(owner_id))
            .collect())
    }

    /// Returns statistics for a collective.
    ///
    /// # Errors
    ///
    /// Returns [`NotFoundError::Collective`] if the collective doesn't exist.
    #[instrument(skip(self))]
    pub fn get_collective_stats(&self, id: CollectiveId) -> Result<CollectiveStats> {
        // Verify collective exists
        self.storage
            .get_collective(id)?
            .ok_or_else(|| PulseDBError::from(NotFoundError::collective(id)))?;

        let experience_count = self.storage.count_experiences_in_collective(id)?;

        Ok(CollectiveStats {
            experience_count,
            storage_bytes: 0,
            oldest_experience: None,
            newest_experience: None,
        })
    }

    /// Deletes a collective and all its associated data.
    ///
    /// Performs cascade deletion: removes all experiences belonging to the
    /// collective before removing the collective record itself.
    ///
    /// # Errors
    ///
    /// Returns [`NotFoundError::Collective`] if the collective doesn't exist.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// db.delete_collective(collective_id)?;
    /// assert!(db.get_collective(collective_id)?.is_none());
    /// ```
    #[instrument(skip(self))]
    pub fn delete_collective(&self, id: CollectiveId) -> Result<()> {
        // Verify collective exists
        self.storage
            .get_collective(id)?
            .ok_or_else(|| PulseDBError::from(NotFoundError::collective(id)))?;

        // Cascade: delete all experiences for this collective
        let deleted_count = self.storage.delete_experiences_by_collective(id)?;
        if deleted_count > 0 {
            info!(count = deleted_count, "Cascade-deleted experiences");
        }

        // Delete the collective record from storage
        self.storage.delete_collective(id)?;

        // Remove HNSW index from memory
        self.vectors
            .write()
            .map_err(|_| PulseDBError::vector("Vectors lock poisoned"))?
            .remove(&id);

        // Remove HNSW files from disk (non-fatal if fails)
        if let Some(hnsw_dir) = self.hnsw_dir() {
            if let Err(e) = HnswIndex::remove_files(&hnsw_dir, &id.to_string()) {
                warn!(
                    collective = %id,
                    error = %e,
                    "Failed to remove HNSW files (non-fatal)"
                );
            }
        }

        info!(id = %id, "Collective deleted");
        Ok(())
    }

    // =========================================================================
    // Experience CRUD (E1-S03)
    // =========================================================================

    /// Records a new experience in the database.
    ///
    /// This is the primary method for storing agent-learned knowledge. The method:
    /// 1. Validates the input (content, scores, tags, embedding)
    /// 2. Verifies the collective exists
    /// 3. Resolves the embedding (generates if Builtin, requires if External)
    /// 4. Stores the experience atomically across 4 tables
    ///
    /// # Arguments
    ///
    /// * `exp` - The experience to record (see [`NewExperience`])
    ///
    /// # Errors
    ///
    /// - [`ValidationError`](crate::ValidationError) if input is invalid
    /// - [`NotFoundError::Collective`] if the collective doesn't exist
    /// - [`PulseDBError::Embedding`] if embedding generation fails (Builtin mode)
    #[instrument(skip(self, exp), fields(collective_id = %exp.collective_id))]
    pub fn record_experience(&self, exp: NewExperience) -> Result<ExperienceId> {
        let is_external = matches!(self.config.embedding_provider, EmbeddingProvider::External);

        // Verify collective exists and get its dimension
        let collective = self
            .storage
            .get_collective(exp.collective_id)?
            .ok_or_else(|| PulseDBError::from(NotFoundError::collective(exp.collective_id)))?;

        // Validate input
        validate_new_experience(&exp, collective.embedding_dimension, is_external)?;

        // Resolve embedding
        let embedding = match exp.embedding {
            Some(emb) => emb,
            None => {
                // Builtin mode: generate embedding from content
                self.embedding.embed(&exp.content)?
            }
        };

        // Clone embedding for HNSW insertion (~1.5KB for 384d, negligible vs I/O)
        let embedding_for_hnsw = embedding.clone();
        let collective_id = exp.collective_id;

        // Construct the full experience record
        let experience = Experience {
            id: ExperienceId::new(),
            collective_id,
            content: exp.content,
            embedding,
            experience_type: exp.experience_type,
            importance: exp.importance,
            confidence: exp.confidence,
            applications: 0,
            domain: exp.domain,
            related_files: exp.related_files,
            source_agent: exp.source_agent,
            source_task: exp.source_task,
            timestamp: Timestamp::now(),
            archived: false,
        };

        let id = experience.id;

        // Write to redb FIRST (source of truth). If crash happens after
        // this but before HNSW insert, rebuild on next open will include it.
        self.storage.save_experience(&experience)?;

        // Insert into HNSW index (derived structure)
        let vectors = self
            .vectors
            .read()
            .map_err(|_| PulseDBError::vector("Vectors lock poisoned"))?;
        if let Some(index) = vectors.get(&collective_id) {
            index.insert_experience(id, &embedding_for_hnsw)?;
        }

        info!(id = %id, "Experience recorded");
        Ok(id)
    }

    /// Retrieves an experience by ID, including its embedding.
    ///
    /// Returns `None` if no experience with the given ID exists.
    #[instrument(skip(self))]
    pub fn get_experience(&self, id: ExperienceId) -> Result<Option<Experience>> {
        self.storage.get_experience(id)
    }

    /// Updates mutable fields of an experience.
    ///
    /// Only fields set to `Some(...)` in the update are changed.
    /// Content and embedding are immutable — create a new experience instead.
    ///
    /// # Errors
    ///
    /// - [`ValidationError`](crate::ValidationError) if updated values are invalid
    /// - [`NotFoundError::Experience`] if the experience doesn't exist
    #[instrument(skip(self, update))]
    pub fn update_experience(&self, id: ExperienceId, update: ExperienceUpdate) -> Result<()> {
        validate_experience_update(&update)?;

        let updated = self.storage.update_experience(id, &update)?;
        if !updated {
            return Err(PulseDBError::from(NotFoundError::experience(id)));
        }

        info!(id = %id, "Experience updated");
        Ok(())
    }

    /// Archives an experience (soft-delete).
    ///
    /// Archived experiences remain in storage but are excluded from search
    /// results. Use [`unarchive_experience`](Self::unarchive_experience) to restore.
    ///
    /// # Errors
    ///
    /// Returns [`NotFoundError::Experience`] if the experience doesn't exist.
    #[instrument(skip(self))]
    pub fn archive_experience(&self, id: ExperienceId) -> Result<()> {
        self.update_experience(
            id,
            ExperienceUpdate {
                archived: Some(true),
                ..Default::default()
            },
        )
    }

    /// Restores an archived experience.
    ///
    /// The experience will once again appear in search results.
    ///
    /// # Errors
    ///
    /// Returns [`NotFoundError::Experience`] if the experience doesn't exist.
    #[instrument(skip(self))]
    pub fn unarchive_experience(&self, id: ExperienceId) -> Result<()> {
        self.update_experience(
            id,
            ExperienceUpdate {
                archived: Some(false),
                ..Default::default()
            },
        )
    }

    /// Permanently deletes an experience and its embedding.
    ///
    /// This removes the experience from all tables and indices.
    /// Unlike archiving, this is irreversible.
    ///
    /// # Errors
    ///
    /// Returns [`NotFoundError::Experience`] if the experience doesn't exist.
    #[instrument(skip(self))]
    pub fn delete_experience(&self, id: ExperienceId) -> Result<()> {
        // Read experience first to get collective_id for HNSW lookup.
        // This adds one extra read, but delete is not a hot path.
        let experience = self
            .storage
            .get_experience(id)?
            .ok_or_else(|| PulseDBError::from(NotFoundError::experience(id)))?;

        // Soft-delete from HNSW index (mark as deleted, not removed from graph)
        let vectors = self
            .vectors
            .read()
            .map_err(|_| PulseDBError::vector("Vectors lock poisoned"))?;
        if let Some(index) = vectors.get(&experience.collective_id) {
            index.delete_experience(id)?;
        }
        drop(vectors);

        // Delete from redb (hard delete)
        self.storage.delete_experience(id)?;

        info!(id = %id, "Experience deleted");
        Ok(())
    }

    /// Reinforces an experience by incrementing its application count.
    ///
    /// Each call atomically increments the `applications` counter by 1.
    /// Returns the new application count.
    ///
    /// # Errors
    ///
    /// Returns [`NotFoundError::Experience`] if the experience doesn't exist.
    #[instrument(skip(self))]
    pub fn reinforce_experience(&self, id: ExperienceId) -> Result<u32> {
        let new_count = self
            .storage
            .reinforce_experience(id)?
            .ok_or_else(|| PulseDBError::from(NotFoundError::experience(id)))?;

        info!(id = %id, applications = new_count, "Experience reinforced");
        Ok(new_count)
    }
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
