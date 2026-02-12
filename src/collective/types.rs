//! Type definitions for collectives.
//!
//! A **collective** is an isolated namespace for experiences, typically one per project.
//! Each collective has its own embedding dimension and vector index.

use serde::{Deserialize, Serialize};

use crate::types::{CollectiveId, Timestamp};

/// A collective — an isolated namespace for agent experiences.
///
/// Collectives provide multi-tenancy: each project or team gets its own
/// collective with its own experiences and vector index.
///
/// # Fields
///
/// - `id` — Unique identifier (UUID v7, time-ordered)
/// - `name` — Human-readable name (e.g., "my-project")
/// - `owner_id` — Optional owner for multi-tenant filtering
/// - `embedding_dimension` — Vector dimension locked at creation (e.g., 384, 768)
/// - `created_at` / `updated_at` — Lifecycle timestamps
///
/// # Serialization
///
/// Collectives are serialized with bincode for compact storage in redb.
/// The `Serialize`/`Deserialize` derives enable this automatically.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Collective {
    /// Unique identifier (UUID v7).
    pub id: CollectiveId,

    /// Human-readable name.
    pub name: String,

    /// Optional owner identifier for multi-tenancy.
    ///
    /// When set, enables filtering collectives by owner via
    /// `list_collectives_by_owner()`.
    pub owner_id: Option<String>,

    /// Embedding vector dimension for this collective.
    ///
    /// All experiences in this collective must have embeddings
    /// with exactly this many dimensions. Locked at creation time.
    pub embedding_dimension: u16,

    /// When this collective was created.
    pub created_at: Timestamp,

    /// When this collective was last modified.
    pub updated_at: Timestamp,
}

impl Collective {
    /// Creates a new collective with the given name and embedding dimension.
    ///
    /// Sets `created_at` and `updated_at` to the current time.
    /// The `owner_id` defaults to `None`.
    pub fn new(name: impl Into<String>, embedding_dimension: u16) -> Self {
        let now = Timestamp::now();
        Self {
            id: CollectiveId::new(),
            name: name.into(),
            owner_id: None,
            embedding_dimension,
            created_at: now,
            updated_at: now,
        }
    }

    /// Creates a new collective with an owner.
    pub fn with_owner(
        name: impl Into<String>,
        owner_id: impl Into<String>,
        embedding_dimension: u16,
    ) -> Self {
        let mut collective = Self::new(name, embedding_dimension);
        collective.owner_id = Some(owner_id.into());
        collective
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collective_new() {
        let collective = Collective::new("test-project", 384);
        assert_eq!(collective.name, "test-project");
        assert_eq!(collective.embedding_dimension, 384);
        assert!(collective.owner_id.is_none());
        assert!(collective.created_at == collective.updated_at);
    }

    #[test]
    fn test_collective_with_owner() {
        let collective = Collective::with_owner("test-project", "user-1", 768);
        assert_eq!(collective.name, "test-project");
        assert_eq!(collective.owner_id.as_deref(), Some("user-1"));
        assert_eq!(collective.embedding_dimension, 768);
    }

    #[test]
    fn test_collective_bincode_roundtrip() {
        let collective = Collective::new("roundtrip-test", 384);
        let bytes = bincode::serialize(&collective).unwrap();
        let restored: Collective = bincode::deserialize(&bytes).unwrap();

        assert_eq!(collective.id, restored.id);
        assert_eq!(collective.name, restored.name);
        assert_eq!(collective.owner_id, restored.owner_id);
        assert_eq!(collective.embedding_dimension, restored.embedding_dimension);
        assert_eq!(collective.created_at, restored.created_at);
        assert_eq!(collective.updated_at, restored.updated_at);
    }

    #[test]
    fn test_collective_bincode_roundtrip_with_owner() {
        let collective = Collective::with_owner("owned-project", "tenant-42", 768);
        let bytes = bincode::serialize(&collective).unwrap();
        let restored: Collective = bincode::deserialize(&bytes).unwrap();

        assert_eq!(collective.id, restored.id);
        assert_eq!(collective.owner_id, restored.owner_id);
    }
}
