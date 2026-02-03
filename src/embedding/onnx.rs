//! ONNX-based embedding generation.
//!
//! This module provides embedding generation using ONNX Runtime.
//! It requires the `builtin-embeddings` feature to be enabled.
//!
//! # Supported Models
//!
//! - **all-MiniLM-L6-v2** (384 dimensions) - Default, fast and compact
//! - **bge-base-en-v1.5** (768 dimensions) - Higher quality, larger
//!
//! # Example
//!
//! ```rust,ignore
//! use pulsedb::embedding::OnnxEmbedding;
//!
//! let service = OnnxEmbedding::new(None)?;  // Use default model
//! let embedding = service.embed("Hello, world!")?;
//! assert_eq!(embedding.len(), 384);
//! ```
//!
//! # Performance Notes
//!
//! - Embedding generation is CPU-intensive
//! - Use `embed_batch()` for multiple texts (more efficient)
//! - Consider using `spawn_blocking` when called from async context

use std::path::PathBuf;

use crate::embedding::EmbeddingService;
use crate::error::{PulseDBError, Result};
use crate::types::Embedding;

/// ONNX-based embedding service.
///
/// This service generates embeddings using an ONNX model loaded via
/// the ONNX Runtime. It supports both the default bundled model and
/// custom models provided by the user.
///
/// # Thread Safety
///
/// `OnnxEmbedding` is `Send + Sync`. ONNX Runtime handles internal
/// synchronization for concurrent inference.
///
/// # Implementation Status
///
/// **TODO**: This is a stub. Full implementation coming in E1-S04.
pub struct OnnxEmbedding {
    /// Path to the ONNX model file.
    #[allow(dead_code)]
    model_path: Option<PathBuf>,

    /// Embedding dimension produced by this model.
    dimension: u16,
}

impl OnnxEmbedding {
    /// Creates a new ONNX embedding service.
    ///
    /// # Arguments
    ///
    /// * `model_path` - Optional path to a custom ONNX model.
    ///   If `None`, uses the bundled all-MiniLM-L6-v2 model.
    ///
    /// # Errors
    ///
    /// Returns an error if the model cannot be loaded.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Use default model
    /// let service = OnnxEmbedding::new(None)?;
    ///
    /// // Use custom model
    /// let service = OnnxEmbedding::new(Some("./my-model.onnx".into()))?;
    /// ```
    pub fn new(model_path: Option<PathBuf>) -> Result<Self> {
        // TODO: Implement in E1-S04
        // - Load ONNX model with ort crate
        // - Initialize tokenizer
        // - Validate model outputs

        // For now, return a stub with default dimension
        Ok(Self {
            model_path,
            dimension: 384, // Default: all-MiniLM-L6-v2
        })
    }

    /// Creates an ONNX embedding service with a specific dimension.
    ///
    /// This is useful for testing or when using a model with known dimension.
    #[cfg(test)]
    pub(crate) fn with_dimension(dimension: u16) -> Self {
        Self {
            model_path: None,
            dimension,
        }
    }
}

impl EmbeddingService for OnnxEmbedding {
    fn embed(&self, _text: &str) -> Result<Embedding> {
        // TODO: Implement in E1-S04
        // 1. Tokenize text
        // 2. Run ONNX inference
        // 3. Pool token embeddings (mean pooling)
        // 4. Normalize to unit length

        Err(PulseDBError::embedding(
            "ONNX embedding not yet implemented (coming in E1-S04)",
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Embedding>> {
        // TODO: Implement in E1-S04
        // - Batch tokenization
        // - Padded inference
        // - Per-text pooling and normalization

        Err(PulseDBError::embedding(
            "ONNX embedding not yet implemented (coming in E1-S04)",
        ))
    }

    fn dimension(&self) -> u16 {
        self.dimension
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_onnx_embedding_new() {
        let service = OnnxEmbedding::new(None).unwrap();
        assert_eq!(service.dimension(), 384);
    }

    #[test]
    fn test_onnx_embedding_with_dimension() {
        let service = OnnxEmbedding::with_dimension(768);
        assert_eq!(service.dimension(), 768);
    }

    #[test]
    fn test_onnx_embedding_embed_not_implemented() {
        let service = OnnxEmbedding::new(None).unwrap();
        let result = service.embed("hello");
        assert!(result.is_err());
    }

    #[test]
    fn test_onnx_embedding_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OnnxEmbedding>();
    }
}
