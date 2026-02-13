//! Collective management module.
//!
//! A **collective** is an isolated namespace for experiences, typically one per project.
//! Each collective has:
//! - Unique ID (UUID v7)
//! - Name and optional owner
//! - Fixed embedding dimension (set at creation)
//! - Its own vector index
//!
//! # Operations
//!
//! All collective operations are available on [`PulseDB`](crate::PulseDB):
//!
//! - [`create_collective(name)`](crate::PulseDB::create_collective)
//! - [`create_collective_with_owner(name, owner_id)`](crate::PulseDB::create_collective_with_owner)
//! - [`get_collective(id)`](crate::PulseDB::get_collective)
//! - [`list_collectives()`](crate::PulseDB::list_collectives)
//! - [`list_collectives_by_owner(owner_id)`](crate::PulseDB::list_collectives_by_owner)
//! - [`get_collective_stats(id)`](crate::PulseDB::get_collective_stats)
//! - [`delete_collective(id)`](crate::PulseDB::delete_collective)
//!
//! # Example
//!
//! ```rust,ignore
//! use pulsedb::{PulseDB, Config};
//!
//! let db = PulseDB::open("./pulse.db", Config::default())?;
//!
//! // Create a collective
//! let id = db.create_collective("my-project")?;
//!
//! // Get collective info
//! if let Some(collective) = db.get_collective(id)? {
//!     println!("Collective: {}", collective.name);
//! }
//!
//! // List all collectives
//! for collective in db.list_collectives()? {
//!     println!("- {}: {}", collective.id, collective.name);
//! }
//!
//! // Get statistics
//! let stats = db.get_collective_stats(id)?;
//! println!("Experiences: {}", stats.experience_count);
//!
//! // Delete when no longer needed
//! db.delete_collective(id)?;
//! ```

pub mod types;

pub use types::{Collective, CollectiveStats};

use crate::error::{PulseDBError, ValidationError};

/// Maximum length for a collective name in characters.
pub const MAX_COLLECTIVE_NAME_LENGTH: usize = 255;

/// Validates a collective name.
///
/// # Rules
///
/// - Must not be empty
/// - Must not be only whitespace
/// - Must not exceed 255 characters
///
/// # Errors
///
/// Returns [`ValidationError::RequiredField`] if empty.
/// Returns [`ValidationError::InvalidField`] if whitespace-only or too long.
pub(crate) fn validate_collective_name(name: &str) -> Result<(), PulseDBError> {
    if name.is_empty() {
        return Err(ValidationError::required_field("name").into());
    }

    if name.trim().is_empty() {
        return Err(ValidationError::invalid_field("name", "must not be only whitespace").into());
    }

    if name.len() > MAX_COLLECTIVE_NAME_LENGTH {
        return Err(ValidationError::invalid_field(
            "name",
            format!(
                "must not exceed {} characters (got {})",
                MAX_COLLECTIVE_NAME_LENGTH,
                name.len()
            ),
        )
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_collective_name_valid() {
        assert!(validate_collective_name("my-project").is_ok());
        assert!(validate_collective_name("a").is_ok());
        assert!(validate_collective_name("Project with spaces").is_ok());
    }

    #[test]
    fn test_validate_collective_name_empty() {
        let err = validate_collective_name("").unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_validate_collective_name_whitespace_only() {
        let err = validate_collective_name("   ").unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_validate_collective_name_too_long() {
        let long_name = "x".repeat(256);
        let err = validate_collective_name(&long_name).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_validate_collective_name_exactly_max_length() {
        let name = "x".repeat(MAX_COLLECTIVE_NAME_LENGTH);
        assert!(validate_collective_name(&name).is_ok());
    }
}
