//! Integration tests for ONNX embedding service.
//!
//! These tests require:
//! 1. The `builtin-embeddings` feature enabled
//! 2. Model files downloaded to the default cache location
//!
//! # Setup
//!
//! ```bash
//! # Download the model (one-time):
//! cargo test --features builtin-embeddings -- --ignored test_download_default_model
//!
//! # Run all integration tests:
//! cargo test --features builtin-embeddings -- --ignored
//! ```

#[cfg(feature = "builtin-embeddings")]
mod onnx_tests {
    use pulsedb::embedding::onnx::OnnxEmbedding;
    use pulsedb::embedding::EmbeddingService;

    /// Check if the default model files are available.
    fn model_available() -> bool {
        // Try to create an OnnxEmbedding — will fail if model not downloaded
        OnnxEmbedding::new(None).is_ok()
    }

    /// Compute cosine similarity between two embeddings.
    /// Since our embeddings are L2-normalized, this is just the dot product.
    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    // ---------------------------------------------------------------
    // Model download test (run first to set up)
    // ---------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_download_default_model() {
        let result = OnnxEmbedding::download_default_model(384);
        match result {
            Ok(path) => {
                println!("Model downloaded to: {}", path.display());
                assert!(path.join("model.onnx").exists());
                assert!(path.join("tokenizer.json").exists());
            }
            Err(e) => {
                // Download might fail due to network — skip gracefully
                eprintln!("Model download failed (network issue?): {e}");
            }
        }
    }

    // ---------------------------------------------------------------
    // Basic functionality tests
    // ---------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_embed_produces_correct_dimension() {
        if !model_available() {
            eprintln!("Skipping: model not available. Run test_download_default_model first.");
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();
        let embedding = service.embed("Hello, world!").unwrap();

        assert_eq!(embedding.len(), 384, "Expected 384-dim embedding");
    }

    #[test]
    #[ignore]
    fn test_embed_is_normalized() {
        if !model_available() {
            eprintln!("Skipping: model not available.");
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();
        let embedding = service.embed("Test normalization").unwrap();

        // L2 norm should be ~1.0
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "Expected unit length, got norm = {norm}"
        );
    }

    #[test]
    #[ignore]
    fn test_embed_empty_text_returns_error() {
        if !model_available() {
            eprintln!("Skipping: model not available.");
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();
        let result = service.embed("");
        assert!(result.is_err(), "Empty text should return error");
    }

    // ---------------------------------------------------------------
    // Semantic similarity tests
    // ---------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_similar_texts_high_cosine_similarity() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();

        let emb_a = service.embed("The cat sat on the mat").unwrap();
        let emb_b = service.embed("A cat was sitting on a mat").unwrap();

        let similarity = cosine_similarity(&emb_a, &emb_b);
        println!("Similar texts cosine similarity: {similarity:.4}");

        assert!(
            similarity > 0.7,
            "Similar texts should have high similarity, got {similarity}"
        );
    }

    #[test]
    #[ignore]
    fn test_different_texts_lower_cosine_similarity() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();

        let emb_a = service.embed("The cat sat on the mat").unwrap();
        let emb_b = service
            .embed("Quantum computing uses qubits for parallel processing")
            .unwrap();

        let similarity = cosine_similarity(&emb_a, &emb_b);
        println!("Different texts cosine similarity: {similarity:.4}");

        assert!(
            similarity < 0.5,
            "Different texts should have lower similarity, got {similarity}"
        );
    }

    // ---------------------------------------------------------------
    // Batch embedding tests
    // ---------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_embed_batch_matches_individual() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();

        let texts = &[
            "Hello world",
            "Rust is great",
            "Machine learning is fascinating",
        ];

        // Embed individually
        let individual: Vec<_> = texts.iter().map(|t| service.embed(t).unwrap()).collect();

        // Embed as batch
        let batch = service.embed_batch(texts).unwrap();

        assert_eq!(batch.len(), texts.len());

        // Batch and individual results should be very close
        // (small floating point differences due to padding in batch mode)
        for (i, (ind, bat)) in individual.iter().zip(batch.iter()).enumerate() {
            let similarity = cosine_similarity(ind, bat);
            assert!(
                similarity > 0.99,
                "Text {i}: batch vs individual similarity = {similarity} (should be > 0.99)"
            );
        }
    }

    #[test]
    #[ignore]
    fn test_embed_batch_empty_returns_empty() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();
        let result = service.embed_batch(&[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    #[ignore]
    fn test_embed_batch_single_item() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();

        let batch = service.embed_batch(&["Single text"]).unwrap();
        let individual = service.embed("Single text").unwrap();

        assert_eq!(batch.len(), 1);
        let similarity = cosine_similarity(&batch[0], &individual);
        assert!(
            similarity > 0.999,
            "Single-item batch should match individual"
        );
    }

    // ---------------------------------------------------------------
    // Edge case tests
    // ---------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_long_text_truncated_not_error() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();

        // Create text longer than max_length (256 tokens for MiniLM)
        let long_text = "word ".repeat(1000); // ~1000 words > 256 tokens
        let result = service.embed(&long_text);
        assert!(result.is_ok(), "Long text should be truncated, not error");
        assert_eq!(result.unwrap().len(), 384);
    }

    #[test]
    #[ignore]
    fn test_special_characters() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();

        // Unicode, punctuation, newlines
        let result = service.embed("Hello! 你好 🌍\nMultiple lines\twith tabs");
        assert!(result.is_ok(), "Special characters should not cause errors");
        assert_eq!(result.unwrap().len(), 384);
    }

    #[test]
    #[ignore]
    fn test_dimension_accessor() {
        if !model_available() {
            return;
        }

        let service = OnnxEmbedding::new(None).unwrap();
        assert_eq!(service.dimension(), 384);
    }

    // ---------------------------------------------------------------
    // Full-stack integration: record_experience with builtin embeddings
    // ---------------------------------------------------------------

    #[test]
    #[ignore]
    fn test_record_experience_builtin_generates_embedding() {
        if !model_available() {
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let config = pulsedb::Config::with_builtin_embeddings();
        let db = pulsedb::PulseDB::open(dir.path(), config).unwrap();

        let cid = db.create_collective("test").unwrap();

        // Record WITHOUT providing an embedding — should auto-generate
        let id = db
            .record_experience(pulsedb::NewExperience {
                collective_id: cid,
                content: "Use Arc for shared ownership across threads".to_string(),
                embedding: None, // <-- No embedding provided!
                importance: 0.9,
                confidence: 0.85,
                ..Default::default()
            })
            .unwrap();

        // Verify embedding was auto-generated
        let exp = db
            .get_experience(id)
            .unwrap()
            .expect("Experience should exist");
        assert_eq!(
            exp.embedding.len(),
            384,
            "Embedding should be 384-dimensional"
        );
        assert!(
            !exp.embedding.iter().all(|&x| x == 0.0),
            "Embedding should not be all zeros"
        );

        // Verify it's normalized
        let norm: f32 = exp.embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "Auto-generated embedding should be normalized, got norm = {norm}"
        );
    }
}
