//! # PulseDB
//!
//! Embedded database for agentic AI systems - the substrate for hive mind architectures.
//!
//! PulseDB provides persistent storage for AI agent experiences, enabling semantic
//! search, context retrieval, and knowledge sharing between agents.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use pulsedb::{PulseDB, Config};
//!
//! // Open or create a database
//! let db = PulseDB::open("./pulse.db", Config::default())?;
//!
//! // Create a collective (isolated namespace)
//! let collective = db.create_collective("my-project")?;
//!
//! // Record an experience
//! db.record_experience(NewExperience {
//!     collective_id: collective,
//!     content: "Always validate user input before processing".to_string(),
//!     experience_type: ExperienceType::Lesson,
//!     importance: 0.8,
//!     ..Default::default()
//! })?;
//!
//! // Search for relevant experiences
//! let results = db.search_similar(collective, &query_embedding, 10)?;
//!
//! // Clean up
//! db.close()?;
//! ```
//!
//! ## Key Concepts
//!
//! ### Collective
//!
//! A **collective** is an isolated namespace for experiences, typically one per project.
//! Each collective has its own vector index and can have different embedding dimensions.
//!
//! ### Experience
//!
//! An **experience** is a unit of learned knowledge. It contains:
//! - Content (text description of the experience)
//! - Embedding (vector representation for semantic search)
//! - Metadata (type, importance, confidence, tags)
//!
//! ### Embedding Providers
//!
//! PulseDB supports two modes for embeddings:
//!
//! - **External** (default): You provide pre-computed embeddings from your own service
//!   (OpenAI, Cohere, etc.)
//! - **Builtin**: PulseDB generates embeddings using a bundled ONNX model
//!   (requires `builtin-embeddings` feature)
//!
//! ## Features
//!
//! - `builtin-embeddings` - Enable built-in ONNX embedding generation
//!
//! ## Thread Safety
//!
//! `PulseDB` is `Send + Sync` and can be shared across threads using `Arc`.
//! The database uses MVCC for concurrent reads with exclusive write locking.

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

// ============================================================================
// Module declarations
// ============================================================================

mod config;
mod db;
mod error;
mod types;

pub mod embedding;
pub mod storage;

// Domain modules
mod collective;
mod experience;
mod search;

/// Vector index module for HNSW-based approximate nearest neighbor search.
pub mod vector;

// ============================================================================
// Public API re-exports
// ============================================================================

// Main database interface
pub use db::PulseDB;

// Configuration
pub use config::{Config, EmbeddingDimension, EmbeddingProvider, HnswConfig, SyncMode};

// Error handling
pub use error::{NotFoundError, PulseDBError, Result, StorageError, ValidationError};

// Core types
pub use types::{AgentId, CollectiveId, Embedding, ExperienceId, TaskId, Timestamp, UserId};

// Domain types
pub use collective::{Collective, CollectiveStats};
pub use experience::{Experience, ExperienceType, ExperienceUpdate, NewExperience, Severity};

// Search
pub use search::SearchFilter;

// Storage (for advanced users)
pub use storage::DatabaseMetadata;

// ============================================================================
// Prelude module for convenient imports
// ============================================================================

/// Convenient imports for common PulseDB usage.
///
/// ```rust
/// use pulsedb::prelude::*;
/// ```
pub mod prelude {
    pub use crate::config::{Config, EmbeddingDimension, SyncMode};
    pub use crate::db::PulseDB;
    pub use crate::error::{PulseDBError, Result};
    pub use crate::experience::{Experience, ExperienceType, NewExperience};
    pub use crate::search::SearchFilter;
    pub use crate::types::{CollectiveId, ExperienceId, Timestamp};
}
