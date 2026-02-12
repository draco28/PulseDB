//! Database schema definitions and versioning.
//!
//! This module defines the table structure for the redb storage engine.
//! All table definitions are compile-time constants to ensure consistency.
//!
//! # Schema Versioning
//!
//! The schema version is stored in the metadata table. When opening an
//! existing database, we check the version and fail if it doesn't match.
//! Migration support will be added in a future release.
//!
//! # Table Layout
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ METADATA_TABLE                                               │
//! │   Key: &str                                                  │
//! │   Value: &[u8] (JSON for human-readable, bincode for data)  │
//! │   Entries: "db_metadata" -> DatabaseMetadata                 │
//! └─────────────────────────────────────────────────────────────┘
//!
//! ┌─────────────────────────────────────────────────────────────┐
//! │ COLLECTIVES_TABLE                                            │
//! │   Key: &[u8; 16] (CollectiveId as UUID bytes)               │
//! │   Value: &[u8] (bincode-serialized Collective)              │
//! └─────────────────────────────────────────────────────────────┘
//!
//! ┌─────────────────────────────────────────────────────────────┐
//! │ EXPERIENCES_TABLE                                            │
//! │   Key: &[u8; 16] (ExperienceId as UUID bytes)               │
//! │   Value: &[u8] (bincode-serialized Experience)              │
//! └─────────────────────────────────────────────────────────────┘
//! ```

use redb::{MultimapTableDefinition, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::config::EmbeddingDimension;
use crate::types::Timestamp;

/// Current schema version.
///
/// Increment this when making breaking changes to the schema.
/// The database will refuse to open if versions don't match.
pub const SCHEMA_VERSION: u32 = 1;

/// Maximum content size in bytes (100 KB).
pub const MAX_CONTENT_SIZE: usize = 100 * 1024;

/// Maximum number of domain tags per experience.
pub const MAX_DOMAIN_TAGS: usize = 10;

/// Maximum length of a single domain tag.
pub const MAX_TAG_LENGTH: usize = 100;

/// Maximum number of source files per experience.
pub const MAX_SOURCE_FILES: usize = 10;

/// Maximum length of a single source file path.
pub const MAX_FILE_PATH_LENGTH: usize = 500;

// ============================================================================
// Table Definitions
// ============================================================================

/// Metadata table for database-level information.
///
/// Stores schema version, creation time, and other database-wide settings.
/// Key is a string identifier, value is serialized data.
pub const METADATA_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");

/// Collectives table.
///
/// Key: CollectiveId as 16-byte UUID
/// Value: bincode-serialized Collective struct
pub const COLLECTIVES_TABLE: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("collectives");

/// Experiences table.
///
/// Key: ExperienceId as 16-byte UUID
/// Value: bincode-serialized Experience struct (without embedding)
pub const EXPERIENCES_TABLE: TableDefinition<&[u8; 16], &[u8]> =
    TableDefinition::new("experiences");

/// Index: Experiences by collective and timestamp.
///
/// Enables efficient queries like "recent experiences in collective X".
/// Key: (CollectiveId bytes, Timestamp big-endian bytes, ExperienceId bytes) = 40 bytes
/// Value: empty (index only)
///
/// Using a multimap allows multiple experiences with the same timestamp.
pub const EXPERIENCES_BY_COLLECTIVE_TABLE: MultimapTableDefinition<&[u8; 16], &[u8; 24]> =
    MultimapTableDefinition::new("experiences_by_collective");

/// Index: Experiences by collective and type.
///
/// Enables efficient queries like "all Lesson experiences in collective X".
/// Key: (CollectiveId bytes, ExperienceTypeTag byte) = 17 bytes
/// Value: ExperienceId as 16-byte UUID
///
/// Using a multimap allows multiple experiences of the same type.
pub const EXPERIENCES_BY_TYPE_TABLE: MultimapTableDefinition<&[u8; 17], &[u8; 16]> =
    MultimapTableDefinition::new("experiences_by_type");

/// Embeddings table.
///
/// Stored separately from experiences to keep the main table compact.
/// Key: ExperienceId as 16-byte UUID
/// Value: raw f32 bytes (dimension * 4 bytes)
pub const EMBEDDINGS_TABLE: TableDefinition<&[u8; 16], &[u8]> = TableDefinition::new("embeddings");

// ============================================================================
// Experience Type Tag
// ============================================================================

/// Compact discriminant for experience types, used in secondary index keys.
///
/// Each variant maps to a single byte (`repr(u8)`), making index keys small
/// and comparison fast. The full `ExperienceType` enum (with any associated
/// data) lives in `experience/types.rs` and will derive its tag via a
/// `type_tag()` method.
///
/// # Variants
///
/// These match the Phase 1 spec's `ExperienceType` enum:
/// - `Observation` — Something the agent noticed
/// - `Decision` — A choice the agent made
/// - `Outcome` — Result of a decision
/// - `Lesson` — A learned principle
/// - `Pattern` — A recurring pattern
/// - `Preference` — A user or agent preference
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ExperienceTypeTag {
    /// Something the agent observed.
    Observation = 0,
    /// A choice the agent made.
    Decision = 1,
    /// Result of a decision or action.
    Outcome = 2,
    /// A learned principle or rule.
    Lesson = 3,
    /// A recurring pattern.
    Pattern = 4,
    /// A user or agent preference.
    Preference = 5,
}

impl ExperienceTypeTag {
    /// Converts a raw byte to an ExperienceTypeTag.
    ///
    /// Returns `None` if the byte doesn't correspond to a known variant.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Observation),
            1 => Some(Self::Decision),
            2 => Some(Self::Outcome),
            3 => Some(Self::Lesson),
            4 => Some(Self::Pattern),
            5 => Some(Self::Preference),
            _ => None,
        }
    }

    /// Returns all variants in discriminant order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Observation,
            Self::Decision,
            Self::Outcome,
            Self::Lesson,
            Self::Pattern,
            Self::Preference,
        ]
    }
}

// ============================================================================
// Database Metadata
// ============================================================================

/// Database metadata stored in the metadata table.
///
/// This is serialized with bincode and stored under the key "db_metadata".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatabaseMetadata {
    /// Schema version for compatibility checking.
    pub schema_version: u32,

    /// Embedding dimension configured for this database.
    ///
    /// Once set, this cannot be changed without recreating the database.
    pub embedding_dimension: EmbeddingDimension,

    /// Timestamp when the database was created.
    pub created_at: Timestamp,

    /// Last time the database was opened (updated on each open).
    pub last_opened_at: Timestamp,
}

impl DatabaseMetadata {
    /// Creates new metadata for a fresh database.
    pub fn new(embedding_dimension: EmbeddingDimension) -> Self {
        let now = Timestamp::now();
        Self {
            schema_version: SCHEMA_VERSION,
            embedding_dimension,
            created_at: now,
            last_opened_at: now,
        }
    }

    /// Updates the last_opened_at timestamp.
    pub fn touch(&mut self) {
        self.last_opened_at = Timestamp::now();
    }

    /// Checks if this metadata is compatible with the current schema.
    pub fn is_compatible(&self) -> bool {
        self.schema_version == SCHEMA_VERSION
    }
}

// ============================================================================
// Key Encoding Helpers
// ============================================================================

/// Encodes a (CollectiveId, Timestamp, ExperienceId) tuple for the index.
///
/// Format: [collective_id: 16 bytes][timestamp_be: 8 bytes] = 24 bytes
/// (ExperienceId is the multimap value, not part of the key)
///
/// Big-endian timestamp ensures lexicographic ordering matches time ordering.
#[inline]
pub fn encode_collective_timestamp_key(collective_id: &[u8; 16], timestamp: Timestamp) -> [u8; 24] {
    let mut key = [0u8; 24];
    key[..16].copy_from_slice(collective_id);
    key[16..24].copy_from_slice(&timestamp.to_be_bytes());
    key
}

/// Decodes the timestamp from a collective index key.
#[inline]
pub fn decode_timestamp_from_key(key: &[u8; 24]) -> Timestamp {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&key[16..24]);
    Timestamp::from_millis(i64::from_be_bytes(bytes))
}

/// Creates a range start key for querying experiences in a collective.
///
/// Uses timestamp 0 (Unix epoch) as the start. We don't support timestamps
/// before 1970 since that predates computers being useful for AI agents.
#[inline]
pub fn collective_range_start(collective_id: &[u8; 16]) -> [u8; 24] {
    encode_collective_timestamp_key(collective_id, Timestamp::from_millis(0))
}

/// Creates a range end key for querying experiences in a collective.
///
/// Uses maximum positive timestamp to include all experiences.
#[inline]
pub fn collective_range_end(collective_id: &[u8; 16]) -> [u8; 24] {
    encode_collective_timestamp_key(collective_id, Timestamp::from_millis(i64::MAX))
}

// ============================================================================
// Type Index Key Encoding
// ============================================================================

/// Encodes a (CollectiveId, ExperienceTypeTag) key for the type index.
///
/// Format: [collective_id: 16 bytes][type_tag: 1 byte] = 17 bytes
///
/// This key design allows efficient range queries: to find all experiences
/// of a given type in a collective, we do a point lookup on this 17-byte key
/// and iterate the multimap values (ExperienceIds).
#[inline]
pub fn encode_type_index_key(collective_id: &[u8; 16], type_tag: ExperienceTypeTag) -> [u8; 17] {
    let mut key = [0u8; 17];
    key[..16].copy_from_slice(collective_id);
    key[16] = type_tag as u8;
    key
}

/// Decodes the ExperienceTypeTag from a type index key.
///
/// Returns `None` if the tag byte doesn't correspond to a known variant.
#[inline]
pub fn decode_type_tag_from_key(key: &[u8; 17]) -> Option<ExperienceTypeTag> {
    ExperienceTypeTag::from_u8(key[16])
}

/// Decodes the CollectiveId bytes from a type index key.
#[inline]
pub fn decode_collective_from_type_key(key: &[u8; 17]) -> [u8; 16] {
    let mut id = [0u8; 16];
    id.copy_from_slice(&key[..16]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_version() {
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn test_database_metadata_new() {
        let meta = DatabaseMetadata::new(EmbeddingDimension::D384);
        assert_eq!(meta.schema_version, SCHEMA_VERSION);
        assert_eq!(meta.embedding_dimension, EmbeddingDimension::D384);
        assert!(meta.is_compatible());
    }

    #[test]
    fn test_database_metadata_touch() {
        let mut meta = DatabaseMetadata::new(EmbeddingDimension::D384);
        let original = meta.last_opened_at;
        std::thread::sleep(std::time::Duration::from_millis(1));
        meta.touch();
        assert!(meta.last_opened_at > original);
    }

    #[test]
    fn test_database_metadata_serialization() {
        let meta = DatabaseMetadata::new(EmbeddingDimension::D768);
        let bytes = bincode::serialize(&meta).unwrap();
        let restored: DatabaseMetadata = bincode::deserialize(&bytes).unwrap();
        assert_eq!(meta.schema_version, restored.schema_version);
        assert_eq!(meta.embedding_dimension, restored.embedding_dimension);
    }

    #[test]
    fn test_encode_collective_timestamp_key() {
        let collective_id = [1u8; 16];
        let timestamp = Timestamp::from_millis(1234567890);

        let key = encode_collective_timestamp_key(&collective_id, timestamp);

        assert_eq!(&key[..16], &collective_id);
        assert_eq!(decode_timestamp_from_key(&key), timestamp);
    }

    #[test]
    fn test_key_ordering() {
        let collective_id = [1u8; 16];
        let t1 = Timestamp::from_millis(1000);
        let t2 = Timestamp::from_millis(2000);

        let key1 = encode_collective_timestamp_key(&collective_id, t1);
        let key2 = encode_collective_timestamp_key(&collective_id, t2);

        // Lexicographic ordering should match timestamp ordering
        assert!(key1 < key2);
    }

    #[test]
    fn test_collective_range() {
        let collective_id = [42u8; 16];
        let start = collective_range_start(&collective_id);
        let end = collective_range_end(&collective_id);

        // Any timestamp should fall within this range
        let mid = encode_collective_timestamp_key(&collective_id, Timestamp::now());
        assert!(start <= mid);
        assert!(mid <= end);
    }

    // ====================================================================
    // ExperienceTypeTag tests
    // ====================================================================

    #[test]
    fn test_experience_type_tag_from_u8_roundtrip() {
        for tag in ExperienceTypeTag::all() {
            let byte = *tag as u8;
            let restored = ExperienceTypeTag::from_u8(byte).unwrap();
            assert_eq!(*tag, restored);
        }
    }

    #[test]
    fn test_experience_type_tag_from_u8_invalid() {
        assert!(ExperienceTypeTag::from_u8(255).is_none());
        assert!(ExperienceTypeTag::from_u8(6).is_none());
    }

    #[test]
    fn test_experience_type_tag_all_variants() {
        let all = ExperienceTypeTag::all();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0], ExperienceTypeTag::Observation);
        assert_eq!(all[5], ExperienceTypeTag::Preference);
    }

    #[test]
    fn test_experience_type_tag_bincode_roundtrip() {
        for tag in ExperienceTypeTag::all() {
            let bytes = bincode::serialize(tag).unwrap();
            let restored: ExperienceTypeTag = bincode::deserialize(&bytes).unwrap();
            assert_eq!(*tag, restored);
        }
    }

    // ====================================================================
    // Type index key encoding tests
    // ====================================================================

    #[test]
    fn test_encode_type_index_key_roundtrip() {
        let collective_id = [7u8; 16];
        let tag = ExperienceTypeTag::Lesson;

        let key = encode_type_index_key(&collective_id, tag);

        assert_eq!(decode_collective_from_type_key(&key), collective_id);
        assert_eq!(decode_type_tag_from_key(&key), Some(tag));
    }

    #[test]
    fn test_type_index_key_different_types_produce_different_keys() {
        let collective_id = [1u8; 16];

        let key_obs = encode_type_index_key(&collective_id, ExperienceTypeTag::Observation);
        let key_les = encode_type_index_key(&collective_id, ExperienceTypeTag::Lesson);

        assert_ne!(key_obs, key_les);
        // Same collective prefix
        assert_eq!(&key_obs[..16], &key_les[..16]);
        // Different type byte
        assert_ne!(key_obs[16], key_les[16]);
    }

    #[test]
    fn test_type_index_key_different_collectives_produce_different_keys() {
        let id_a = [1u8; 16];
        let id_b = [2u8; 16];
        let tag = ExperienceTypeTag::Decision;

        let key_a = encode_type_index_key(&id_a, tag);
        let key_b = encode_type_index_key(&id_b, tag);

        assert_ne!(key_a, key_b);
        // Same type byte
        assert_eq!(key_a[16], key_b[16]);
    }
}
