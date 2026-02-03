//! Configuration types for PulseDB.
//!
//! The [`Config`] struct controls database behavior including:
//! - Embedding provider (builtin ONNX or external)
//! - Embedding dimension (384, 768, or custom)
//! - Cache size and durability settings
//!
//! # Example
//! ```rust
//! use pulsedb::{Config, EmbeddingProvider, EmbeddingDimension, SyncMode};
//!
//! // Use defaults (External provider, 384 dimensions)
//! let config = Config::default();
//!
//! // Customize for production
//! let config = Config {
//!     embedding_dimension: EmbeddingDimension::D768,
//!     cache_size_mb: 128,
//!     sync_mode: SyncMode::Normal,
//!     ..Default::default()
//! };
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::ValidationError;
use crate::types::CollectiveId;

/// Database configuration options.
///
/// All fields have sensible defaults. Use struct update syntax to override
/// specific settings:
///
/// ```rust
/// use pulsedb::Config;
///
/// let config = Config {
///     cache_size_mb: 256,
///     ..Default::default()
/// };
/// ```
#[derive(Clone, Debug)]
pub struct Config {
    /// How embeddings are generated or provided.
    pub embedding_provider: EmbeddingProvider,

    /// Embedding vector dimension (must match provider output).
    pub embedding_dimension: EmbeddingDimension,

    /// Default collective for operations when none specified.
    pub default_collective: Option<CollectiveId>,

    /// Cache size in megabytes for the storage engine.
    ///
    /// Higher values improve read performance but use more memory.
    /// Default: 64 MB
    pub cache_size_mb: usize,

    /// Durability mode for write operations.
    pub sync_mode: SyncMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // External is the safe default - no ONNX dependency required
            embedding_provider: EmbeddingProvider::External,
            // 384 matches all-MiniLM-L6-v2, the default builtin model
            embedding_dimension: EmbeddingDimension::D384,
            default_collective: None,
            cache_size_mb: 64,
            sync_mode: SyncMode::Normal,
        }
    }
}

impl Config {
    /// Creates a new Config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a Config for builtin embedding generation.
    ///
    /// This requires the `builtin-embeddings` feature to be enabled.
    ///
    /// # Example
    /// ```rust
    /// use pulsedb::Config;
    ///
    /// let config = Config::with_builtin_embeddings();
    /// ```
    pub fn with_builtin_embeddings() -> Self {
        Self {
            embedding_provider: EmbeddingProvider::Builtin { model_path: None },
            ..Default::default()
        }
    }

    /// Creates a Config for external embedding provider.
    ///
    /// When using external embeddings, you must provide pre-computed
    /// embedding vectors when recording experiences.
    ///
    /// # Example
    /// ```rust
    /// use pulsedb::{Config, EmbeddingDimension};
    ///
    /// // OpenAI ada-002 uses 1536 dimensions
    /// let config = Config::with_external_embeddings(EmbeddingDimension::Custom(1536));
    /// ```
    pub fn with_external_embeddings(dimension: EmbeddingDimension) -> Self {
        Self {
            embedding_provider: EmbeddingProvider::External,
            embedding_dimension: dimension,
            ..Default::default()
        }
    }

    /// Validates the configuration.
    ///
    /// Called automatically by `PulseDB::open()`. You can also call this
    /// explicitly to check configuration before attempting to open.
    ///
    /// # Errors
    /// Returns `ValidationError` if:
    /// - `cache_size_mb` is 0
    /// - Custom dimension is 0 or > 4096
    pub fn validate(&self) -> Result<(), ValidationError> {
        // Cache size must be positive
        if self.cache_size_mb == 0 {
            return Err(ValidationError::invalid_field(
                "cache_size_mb",
                "must be greater than 0",
            ));
        }

        // Validate custom dimension bounds
        if let EmbeddingDimension::Custom(dim) = self.embedding_dimension {
            if dim == 0 {
                return Err(ValidationError::invalid_field(
                    "embedding_dimension",
                    "custom dimension must be greater than 0",
                ));
            }
            if dim > 4096 {
                return Err(ValidationError::invalid_field(
                    "embedding_dimension",
                    "custom dimension must not exceed 4096",
                ));
            }
        }

        Ok(())
    }

    /// Returns the embedding dimension as a numeric value.
    pub fn dimension(&self) -> usize {
        self.embedding_dimension.size()
    }
}

/// Embedding provider configuration.
///
/// Determines how embedding vectors are generated for experiences.
#[derive(Clone, Debug)]
pub enum EmbeddingProvider {
    /// PulseDB generates embeddings using a built-in ONNX model.
    ///
    /// Requires the `builtin-embeddings` feature. The default model is
    /// all-MiniLM-L6-v2 (384 dimensions).
    Builtin {
        /// Custom ONNX model path. If `None`, uses the bundled model.
        model_path: Option<PathBuf>,
    },

    /// Caller provides pre-computed embedding vectors.
    ///
    /// Use this when you have your own embedding service (OpenAI, Cohere, etc.)
    /// or want to use a model not bundled with PulseDB.
    External,
}

impl EmbeddingProvider {
    /// Returns true if this is the builtin provider.
    pub fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin { .. })
    }

    /// Returns true if this is the external provider.
    pub fn is_external(&self) -> bool {
        matches!(self, Self::External)
    }
}

/// Embedding vector dimensions.
///
/// Standard dimensions are provided for common models. Use `Custom` for
/// other embedding services.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmbeddingDimension {
    /// 384 dimensions (all-MiniLM-L6-v2, default builtin model).
    #[default]
    D384,

    /// 768 dimensions (bge-base-en-v1.5, BERT-base).
    D768,

    /// Custom dimension for other embedding models.
    ///
    /// Must be between 1 and 4096.
    Custom(usize),
}

impl EmbeddingDimension {
    /// Returns the numeric size of this dimension.
    ///
    /// # Example
    /// ```rust
    /// use pulsedb::EmbeddingDimension;
    ///
    /// assert_eq!(EmbeddingDimension::D384.size(), 384);
    /// assert_eq!(EmbeddingDimension::D768.size(), 768);
    /// assert_eq!(EmbeddingDimension::Custom(1536).size(), 1536);
    /// ```
    #[inline]
    pub const fn size(&self) -> usize {
        match self {
            Self::D384 => 384,
            Self::D768 => 768,
            Self::Custom(n) => *n,
        }
    }
}

/// Durability mode for write operations.
///
/// Controls the trade-off between write performance and crash safety.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncMode {
    /// Sync to disk on transaction commit.
    ///
    /// This is the default and recommended setting. Provides good performance
    /// while ensuring committed data survives crashes.
    #[default]
    Normal,

    /// Async sync (faster writes, may lose recent data on crash).
    ///
    /// Use for development or when you can tolerate losing the last few
    /// seconds of writes. Significantly faster than `Normal`.
    Fast,

    /// Sync every write operation (slowest, maximum durability).
    ///
    /// Use when data loss is absolutely unacceptable. Very slow for
    /// high write volumes.
    Paranoid,
}

impl SyncMode {
    /// Returns true if this mode syncs on every write.
    pub fn is_paranoid(&self) -> bool {
        matches!(self, Self::Paranoid)
    }

    /// Returns true if this mode is async (may lose data on crash).
    pub fn is_fast(&self) -> bool {
        matches!(self, Self::Fast)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.embedding_provider.is_external());
        assert_eq!(config.embedding_dimension, EmbeddingDimension::D384);
        assert_eq!(config.cache_size_mb, 64);
        assert_eq!(config.sync_mode, SyncMode::Normal);
        assert!(config.default_collective.is_none());
    }

    #[test]
    fn test_with_builtin_embeddings() {
        let config = Config::with_builtin_embeddings();
        assert!(config.embedding_provider.is_builtin());
    }

    #[test]
    fn test_with_external_embeddings() {
        let config = Config::with_external_embeddings(EmbeddingDimension::Custom(1536));
        assert!(config.embedding_provider.is_external());
        assert_eq!(config.dimension(), 1536);
    }

    #[test]
    fn test_validate_success() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_cache_size_zero() {
        let config = Config {
            cache_size_mb: 0,
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(
            matches!(err, ValidationError::InvalidField { field, .. } if field == "cache_size_mb")
        );
    }

    #[test]
    fn test_validate_custom_dimension_zero() {
        let config = Config {
            embedding_dimension: EmbeddingDimension::Custom(0),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_custom_dimension_too_large() {
        let config = Config {
            embedding_dimension: EmbeddingDimension::Custom(5000),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_custom_dimension_valid() {
        let config = Config {
            embedding_dimension: EmbeddingDimension::Custom(1536),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_embedding_dimension_sizes() {
        assert_eq!(EmbeddingDimension::D384.size(), 384);
        assert_eq!(EmbeddingDimension::D768.size(), 768);
        assert_eq!(EmbeddingDimension::Custom(512).size(), 512);
    }

    #[test]
    fn test_sync_mode_checks() {
        assert!(!SyncMode::Normal.is_fast());
        assert!(!SyncMode::Normal.is_paranoid());
        assert!(SyncMode::Fast.is_fast());
        assert!(SyncMode::Paranoid.is_paranoid());
    }

    #[test]
    fn test_embedding_dimension_serialization() {
        let dim = EmbeddingDimension::D768;
        let bytes = bincode::serialize(&dim).unwrap();
        let restored: EmbeddingDimension = bincode::deserialize(&bytes).unwrap();
        assert_eq!(dim, restored);
    }
}
