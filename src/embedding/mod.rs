//! Embedding service abstractions for PulseDB.
//!
//! This module provides the trait and implementations for embedding generation.
//! Embeddings are dense vector representations of text used for semantic search.
//!
//! # Providers
//!
//! - [`ExternalEmbedding`] - For pre-computed embeddings (e.g., OpenAI, Cohere)
//! - `OnnxEmbedding` - Built-in ONNX model (requires `builtin-embeddings` feature)
//!
//! # Example
//!
//! ```rust,ignore
//! use pulsedb::embedding::{EmbeddingService, ExternalEmbedding};
//!
//! // External mode - user provides embeddings
//! let service = ExternalEmbedding::new(384);
//! assert_eq!(service.dimension(), 384);
//!
//! // Validation only - cannot generate embeddings
//! let result = service.embed("hello");
//! assert!(result.is_err());
//! ```

#[cfg(feature = "builtin-embeddings")]
pub mod onnx;

use crate::error::{PulseDBError, Result};
use crate::types::Embedding;

/// Embedding service trait for generating vector representations of text.
///
/// This trait defines the contract for any embedding provider. Implementations
/// must be thread-safe (`Send + Sync`) to allow concurrent embedding operations.
///
/// # Implementing a Custom Provider
///
/// ```rust,ignore
/// use pulsedb::embedding::EmbeddingService;
/// use pulsedb::{Embedding, Result};
///
/// struct MyEmbeddingService {
///     client: MyApiClient,
///     dimension: u16,
/// }
///
/// impl EmbeddingService for MyEmbeddingService {
///     fn embed(&self, text: &str) -> Result<Embedding> {
///         Ok(self.client.get_embedding(text)?)
///     }
///
///     fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
///         Ok(self.client.get_embeddings(texts)?)
///     }
///
///     fn dimension(&self) -> u16 {
///         self.dimension
///     }
/// }
/// ```
pub trait EmbeddingService: Send + Sync {
    /// Generates an embedding for a single text.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to embed
    ///
    /// # Returns
    ///
    /// A vector of f32 values with length equal to `dimension()`.
    ///
    /// # Errors
    ///
    /// Returns `PulseDBError::Embedding` if embedding generation fails.
    fn embed(&self, text: &str) -> Result<Embedding>;

    /// Generates embeddings for multiple texts in a batch.
    ///
    /// Batch processing is typically more efficient than individual calls
    /// due to reduced API overhead and better GPU utilization.
    ///
    /// # Arguments
    ///
    /// * `texts` - Slice of texts to embed
    ///
    /// # Returns
    ///
    /// A vector of embeddings in the same order as the input texts.
    ///
    /// # Errors
    ///
    /// Returns `PulseDBError::Embedding` if any embedding generation fails.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>>;

    /// Returns the dimension of embeddings produced by this service.
    ///
    /// All embeddings from this service will have exactly this many dimensions.
    fn dimension(&self) -> u16;

    /// Validates that an embedding has the correct dimension.
    ///
    /// # Errors
    ///
    /// Returns `ValidationError::DimensionMismatch` if dimensions don't match.
    fn validate_embedding(&self, embedding: &Embedding) -> Result<()> {
        let expected = self.dimension() as usize;
        let actual = embedding.len();

        if actual != expected {
            return Err(PulseDBError::Validation(
                crate::error::ValidationError::dimension_mismatch(expected, actual),
            ));
        }

        Ok(())
    }
}

/// External embedding provider.
///
/// This provider is used when embeddings are generated externally (e.g., by
/// OpenAI, Cohere, or a custom service). It validates embedding dimensions
/// but cannot generate embeddings itself.
///
/// # Usage
///
/// When using `ExternalEmbedding`, you must provide pre-computed embedding
/// vectors when recording experiences. Attempting to call `embed()` or
/// `embed_batch()` will return an error.
///
/// # Example
///
/// ```rust
/// use pulsedb::embedding::{EmbeddingService, ExternalEmbedding};
///
/// // Create for OpenAI ada-002 (1536 dimensions)
/// let service = ExternalEmbedding::new(1536);
/// assert_eq!(service.dimension(), 1536);
/// ```
#[derive(Clone, Debug)]
pub struct ExternalEmbedding {
    dimension: u16,
}

impl ExternalEmbedding {
    /// Creates a new external embedding provider with the given dimension.
    ///
    /// # Arguments
    ///
    /// * `dimension` - The expected embedding dimension
    ///
    /// # Example
    ///
    /// ```rust
    /// use pulsedb::embedding::ExternalEmbedding;
    ///
    /// // all-MiniLM-L6-v2
    /// let service = ExternalEmbedding::new(384);
    ///
    /// // OpenAI text-embedding-3-small
    /// let service = ExternalEmbedding::new(1536);
    /// ```
    pub fn new(dimension: u16) -> Self {
        Self { dimension }
    }
}

impl EmbeddingService for ExternalEmbedding {
    fn embed(&self, _text: &str) -> Result<Embedding> {
        Err(PulseDBError::embedding(
            "External embedding mode: embeddings must be provided by the caller",
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Embedding>> {
        Err(PulseDBError::embedding(
            "External embedding mode: embeddings must be provided by the caller",
        ))
    }

    fn dimension(&self) -> u16 {
        self.dimension
    }
}

/// Creates an embedding service based on the configuration.
///
/// # Arguments
///
/// * `config` - Database configuration specifying the embedding provider
///
/// # Returns
///
/// A boxed embedding service ready for use.
///
/// # Errors
///
/// Returns an error if:
/// - Builtin embeddings requested but feature not enabled
/// - ONNX model loading fails (for builtin provider)
pub fn create_embedding_service(
    config: &crate::config::Config,
) -> Result<Box<dyn EmbeddingService>> {
    use crate::config::EmbeddingProvider;

    match &config.embedding_provider {
        EmbeddingProvider::External => {
            let dimension = config.embedding_dimension.size() as u16;
            Ok(Box::new(ExternalEmbedding::new(dimension)))
        }

        #[cfg(feature = "builtin-embeddings")]
        EmbeddingProvider::Builtin { model_path } => {
            // TODO: Implement in E1-S04
            let _ = model_path;
            Err(PulseDBError::embedding(
                "Builtin embeddings not yet implemented (coming in E1-S04)",
            ))
        }

        #[cfg(not(feature = "builtin-embeddings"))]
        EmbeddingProvider::Builtin { .. } => Err(PulseDBError::embedding(
            "Builtin embeddings require the 'builtin-embeddings' feature",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_external_embedding_dimension() {
        let service = ExternalEmbedding::new(384);
        assert_eq!(service.dimension(), 384);
    }

    #[test]
    fn test_external_embedding_embed_returns_error() {
        let service = ExternalEmbedding::new(384);
        let result = service.embed("hello world");
        assert!(result.is_err());
    }

    #[test]
    fn test_external_embedding_embed_batch_returns_error() {
        let service = ExternalEmbedding::new(384);
        let result = service.embed_batch(&["hello", "world"]);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_embedding_correct_dimension() {
        let service = ExternalEmbedding::new(3);
        let embedding = vec![1.0, 2.0, 3.0];
        assert!(service.validate_embedding(&embedding).is_ok());
    }

    #[test]
    fn test_validate_embedding_wrong_dimension() {
        let service = ExternalEmbedding::new(3);
        let embedding = vec![1.0, 2.0]; // Only 2 dimensions
        let result = service.validate_embedding(&embedding);
        assert!(result.is_err());
    }

    #[test]
    fn test_external_embedding_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ExternalEmbedding>();
    }

    #[test]
    fn test_create_embedding_service_external() {
        let config = crate::config::Config::default();
        let service = create_embedding_service(&config).unwrap();
        assert_eq!(service.dimension(), 384);
    }

    #[test]
    fn test_create_embedding_service_builtin_not_implemented() {
        let config = crate::config::Config::with_builtin_embeddings();
        let result = create_embedding_service(&config);
        // Should fail because builtin is not yet implemented
        assert!(result.is_err());
    }
}
