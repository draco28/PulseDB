//! Error types for PulseDB.
//!
//! PulseDB uses a hierarchical error system:
//! - `PulseDBError` is the top-level error returned by all public APIs
//! - Specific error types (`StorageError`, `ValidationError`) provide detail
//!
//! # Error Handling Pattern
//! ```rust,ignore
//! use pulsedb::{PulseDB, Config, Result};
//!
//! fn example() -> Result<()> {
//!     let db = PulseDB::open("./pulse.db", Config::default())?;
//!     // ... operations that may fail ...
//!     db.close()?;
//!     Ok(())
//! }
//! ```

use std::path::PathBuf;
use thiserror::Error;

/// Result type alias for PulseDB operations.
pub type Result<T> = std::result::Result<T, PulseDBError>;

/// Top-level error enum for all PulseDB operations.
///
/// This is the only error type returned by public APIs.
/// Use pattern matching to handle specific error cases.
#[derive(Debug, Error)]
pub enum PulseDBError {
    /// Storage layer error (I/O, corruption, transactions).
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Input validation error.
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    /// Configuration error.
    #[error("Configuration error: {reason}")]
    Config {
        /// Description of what's wrong with the configuration.
        reason: String,
    },

    /// Requested entity not found.
    #[error("{0}")]
    NotFound(#[from] NotFoundError),

    /// General I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Embedding generation/validation error.
    #[error("Embedding error: {0}")]
    Embedding(String),

    /// Vector index error (HNSW operations).
    #[error("Vector index error: {0}")]
    Vector(String),
}

impl PulseDBError {
    /// Creates a configuration error with the given reason.
    pub fn config(reason: impl Into<String>) -> Self {
        Self::Config {
            reason: reason.into(),
        }
    }

    /// Creates an embedding error with the given message.
    pub fn embedding(msg: impl Into<String>) -> Self {
        Self::Embedding(msg.into())
    }

    /// Creates a vector index error with the given message.
    pub fn vector(msg: impl Into<String>) -> Self {
        Self::Vector(msg.into())
    }

    /// Returns true if this is a "not found" error.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }

    /// Returns true if this is a validation error.
    pub fn is_validation(&self) -> bool {
        matches!(self, Self::Validation(_))
    }

    /// Returns true if this is a storage error.
    pub fn is_storage(&self) -> bool {
        matches!(self, Self::Storage(_))
    }

    /// Returns true if this is a vector index error.
    pub fn is_vector(&self) -> bool {
        matches!(self, Self::Vector(_))
    }
}

/// Storage-related errors.
///
/// These errors indicate problems with the underlying storage layer.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Database file or data is corrupted.
    #[error("Database corrupted: {0}")]
    Corrupted(String),

    /// Database file not found at expected path.
    #[error("Database not found: {0}")]
    DatabaseNotFound(PathBuf),

    /// Database is locked by another process.
    #[error("Database is locked by another writer")]
    DatabaseLocked,

    /// Transaction failed (commit, rollback, etc.).
    #[error("Transaction failed: {0}")]
    Transaction(String),

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Error from the redb storage engine.
    #[error("Storage engine error: {0}")]
    Redb(String),

    /// Database schema version doesn't match expected version.
    #[error("Schema version mismatch: expected {expected}, found {found}")]
    SchemaVersionMismatch {
        /// Expected schema version.
        expected: u32,
        /// Actual schema version found in database.
        found: u32,
    },

    /// Table not found in database.
    #[error("Table not found: {0}")]
    TableNotFound(String),
}

impl StorageError {
    /// Creates a corruption error with the given message.
    pub fn corrupted(msg: impl Into<String>) -> Self {
        Self::Corrupted(msg.into())
    }

    /// Creates a transaction error with the given message.
    pub fn transaction(msg: impl Into<String>) -> Self {
        Self::Transaction(msg.into())
    }

    /// Creates a serialization error with the given message.
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }

    /// Creates a redb error with the given message.
    pub fn redb(msg: impl Into<String>) -> Self {
        Self::Redb(msg.into())
    }
}

// Conversions from redb error types
impl From<redb::Error> for StorageError {
    fn from(err: redb::Error) -> Self {
        StorageError::Redb(err.to_string())
    }
}

impl From<redb::DatabaseError> for StorageError {
    fn from(err: redb::DatabaseError) -> Self {
        StorageError::Redb(err.to_string())
    }
}

impl From<redb::TransactionError> for StorageError {
    fn from(err: redb::TransactionError) -> Self {
        StorageError::Transaction(err.to_string())
    }
}

impl From<redb::CommitError> for StorageError {
    fn from(err: redb::CommitError) -> Self {
        StorageError::Transaction(format!("Commit failed: {}", err))
    }
}

impl From<redb::TableError> for StorageError {
    fn from(err: redb::TableError) -> Self {
        StorageError::Redb(format!("Table error: {}", err))
    }
}

impl From<redb::StorageError> for StorageError {
    fn from(err: redb::StorageError) -> Self {
        StorageError::Redb(format!("Storage error: {}", err))
    }
}

// Convert bincode errors to StorageError
impl From<bincode::Error> for StorageError {
    fn from(err: bincode::Error) -> Self {
        StorageError::Serialization(err.to_string())
    }
}

// Also allow direct conversion to PulseDBError for convenience
impl From<redb::Error> for PulseDBError {
    fn from(err: redb::Error) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

impl From<redb::DatabaseError> for PulseDBError {
    fn from(err: redb::DatabaseError) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

impl From<redb::TransactionError> for PulseDBError {
    fn from(err: redb::TransactionError) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

impl From<redb::CommitError> for PulseDBError {
    fn from(err: redb::CommitError) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

impl From<redb::TableError> for PulseDBError {
    fn from(err: redb::TableError) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

impl From<redb::StorageError> for PulseDBError {
    fn from(err: redb::StorageError) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

impl From<bincode::Error> for PulseDBError {
    fn from(err: bincode::Error) -> Self {
        PulseDBError::Storage(StorageError::from(err))
    }
}

/// Validation errors for input data.
///
/// These errors indicate problems with data provided by the caller.
#[derive(Debug, Error)]
pub enum ValidationError {
    /// Embedding dimension doesn't match collective's configured dimension.
    #[error("Embedding dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch {
        /// Expected dimension from collective configuration.
        expected: usize,
        /// Actual dimension provided.
        got: usize,
    },

    /// A field has an invalid value.
    #[error("Invalid field '{field}': {reason}")]
    InvalidField {
        /// Name of the invalid field.
        field: String,
        /// Why the value is invalid.
        reason: String,
    },

    /// Content exceeds maximum allowed size.
    #[error("Content too large: {size} bytes (max: {max} bytes)")]
    ContentTooLarge {
        /// Actual content size in bytes.
        size: usize,
        /// Maximum allowed size in bytes.
        max: usize,
    },

    /// A required field is missing or empty.
    #[error("Required field missing: {field}")]
    RequiredField {
        /// Name of the missing field.
        field: String,
    },

    /// Too many items in a collection field.
    #[error("Too many items in '{field}': {count} (max: {max})")]
    TooManyItems {
        /// Name of the field.
        field: String,
        /// Actual count.
        count: usize,
        /// Maximum allowed.
        max: usize,
    },
}

impl ValidationError {
    /// Creates a dimension mismatch error.
    pub fn dimension_mismatch(expected: usize, got: usize) -> Self {
        Self::DimensionMismatch { expected, got }
    }

    /// Creates an invalid field error.
    pub fn invalid_field(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidField {
            field: field.into(),
            reason: reason.into(),
        }
    }

    /// Creates a content too large error.
    pub fn content_too_large(size: usize, max: usize) -> Self {
        Self::ContentTooLarge { size, max }
    }

    /// Creates a required field error.
    pub fn required_field(field: impl Into<String>) -> Self {
        Self::RequiredField {
            field: field.into(),
        }
    }

    /// Creates a too many items error.
    pub fn too_many_items(field: impl Into<String>, count: usize, max: usize) -> Self {
        Self::TooManyItems {
            field: field.into(),
            count,
            max,
        }
    }
}

/// Not found errors for specific entity types.
#[derive(Debug, Error)]
pub enum NotFoundError {
    /// Collective with given ID not found.
    #[error("Collective not found: {0}")]
    Collective(String),

    /// Experience with given ID not found.
    #[error("Experience not found: {0}")]
    Experience(String),

    /// Relation with given ID not found.
    #[error("Relation not found: {0}")]
    Relation(String),

    /// Insight with given ID not found.
    #[error("Insight not found: {0}")]
    Insight(String),

    /// Activity not found for given agent/collective pair.
    #[error("Activity not found: {0}")]
    Activity(String),
}

impl NotFoundError {
    /// Creates a collective not found error.
    pub fn collective(id: impl ToString) -> Self {
        Self::Collective(id.to_string())
    }

    /// Creates an experience not found error.
    pub fn experience(id: impl ToString) -> Self {
        Self::Experience(id.to_string())
    }

    /// Creates a relation not found error.
    pub fn relation(id: impl ToString) -> Self {
        Self::Relation(id.to_string())
    }

    /// Creates an insight not found error.
    pub fn insight(id: impl ToString) -> Self {
        Self::Insight(id.to_string())
    }

    /// Creates an activity not found error.
    pub fn activity(id: impl ToString) -> Self {
        Self::Activity(id.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = PulseDBError::config("Invalid dimension");
        assert_eq!(err.to_string(), "Configuration error: Invalid dimension");
    }

    #[test]
    fn test_storage_error_display() {
        let err = StorageError::SchemaVersionMismatch {
            expected: 2,
            found: 1,
        };
        assert_eq!(
            err.to_string(),
            "Schema version mismatch: expected 2, found 1"
        );
    }

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError::dimension_mismatch(384, 768);
        assert_eq!(
            err.to_string(),
            "Embedding dimension mismatch: expected 384, got 768"
        );
    }

    #[test]
    fn test_not_found_error_display() {
        let err = NotFoundError::collective("abc-123");
        assert_eq!(err.to_string(), "Collective not found: abc-123");
    }

    #[test]
    fn test_is_not_found() {
        let err: PulseDBError = NotFoundError::collective("test").into();
        assert!(err.is_not_found());
        assert!(!err.is_validation());
    }

    #[test]
    fn test_is_validation() {
        let err: PulseDBError = ValidationError::required_field("content").into();
        assert!(err.is_validation());
        assert!(!err.is_not_found());
    }

    #[test]
    fn test_vector_error_display() {
        let err = PulseDBError::vector("HNSW insert failed");
        assert_eq!(err.to_string(), "Vector index error: HNSW insert failed");
        assert!(err.is_vector());
        assert!(!err.is_storage());
    }

    #[test]
    fn test_error_conversion_chain() {
        // Simulate a storage error propagating up
        fn inner() -> Result<()> {
            Err(StorageError::corrupted("test corruption"))?
        }

        let result = inner();
        assert!(result.is_err());
        assert!(result.unwrap_err().is_storage());
    }
}
