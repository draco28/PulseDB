//! Relation management module.
//!
//! A **relation** connects two experiences within the same collective,
//! forming a knowledge graph that enables agents to understand how
//! concepts relate to each other.
//!
//! # Operations
//!
//! All relation operations are available on [`PulseDB`](crate::PulseDB):
//!
//! - [`store_relation(rel)`](crate::PulseDB::store_relation)
//! - [`get_related_experiences(id, direction)`](crate::PulseDB::get_related_experiences)
//! - [`get_relation(id)`](crate::PulseDB::get_relation)
//! - [`delete_relation(id)`](crate::PulseDB::delete_relation)
//!
//! # Constraints
//!
//! - Relations cannot be self-referential (`source_id != target_id`)
//! - Both experiences must belong to the same collective
//! - The `(source_id, target_id, relation_type)` triple must be unique
//! - Strength must be in `[0.0, 1.0]`
//! - Metadata must be ≤ 10KB

pub mod types;

pub use types::{ExperienceRelation, NewExperienceRelation, RelationDirection, RelationType};

use crate::error::{PulseDBError, ValidationError};
use crate::storage::schema::MAX_RELATION_METADATA_SIZE;

/// Validates a new relation before storage.
///
/// Checks:
/// - Source and target are different experiences (no self-relations)
/// - Strength is in the valid range [0.0, 1.0]
/// - Metadata (if provided) doesn't exceed 10KB
///
/// Does NOT check cross-collective or duplicate constraints — those
/// require storage lookups and are handled by the PulseDB facade.
pub(crate) fn validate_new_relation(rel: &NewExperienceRelation) -> Result<(), PulseDBError> {
    // Self-relation check
    if rel.source_id == rel.target_id {
        return Err(ValidationError::invalid_field(
            "target_id",
            "cannot create a self-relation (source_id == target_id)",
        )
        .into());
    }

    // Strength range
    if !(0.0..=1.0).contains(&rel.strength) {
        return Err(ValidationError::invalid_field(
            "strength",
            format!("must be between 0.0 and 1.0, got {}", rel.strength),
        )
        .into());
    }

    // Metadata size
    if let Some(ref metadata) = rel.metadata {
        if metadata.len() > MAX_RELATION_METADATA_SIZE {
            return Err(ValidationError::content_too_large(
                metadata.len(),
                MAX_RELATION_METADATA_SIZE,
            )
            .into());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExperienceId;

    fn valid_new_relation() -> NewExperienceRelation {
        NewExperienceRelation {
            source_id: ExperienceId::new(),
            target_id: ExperienceId::new(),
            relation_type: RelationType::Supports,
            strength: 0.8,
            metadata: None,
        }
    }

    #[test]
    fn test_valid_relation_passes() {
        let rel = valid_new_relation();
        assert!(validate_new_relation(&rel).is_ok());
    }

    #[test]
    fn test_self_relation_rejected() {
        let id = ExperienceId::new();
        let rel = NewExperienceRelation {
            source_id: id,
            target_id: id, // Same!
            relation_type: RelationType::Supports,
            strength: 0.5,
            metadata: None,
        };
        let err = validate_new_relation(&rel).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("self-relation"));
    }

    #[test]
    fn test_strength_below_zero_rejected() {
        let mut rel = valid_new_relation();
        rel.strength = -0.1;
        let err = validate_new_relation(&rel).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("strength"));
    }

    #[test]
    fn test_strength_above_one_rejected() {
        let mut rel = valid_new_relation();
        rel.strength = 1.1;
        let err = validate_new_relation(&rel).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_strength_boundary_values() {
        let mut rel = valid_new_relation();

        rel.strength = 0.0;
        assert!(validate_new_relation(&rel).is_ok());

        rel.strength = 1.0;
        assert!(validate_new_relation(&rel).is_ok());
    }

    #[test]
    fn test_metadata_too_large_rejected() {
        let mut rel = valid_new_relation();
        rel.metadata = Some("x".repeat(MAX_RELATION_METADATA_SIZE + 1));
        let err = validate_new_relation(&rel).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn test_metadata_at_limit_passes() {
        let mut rel = valid_new_relation();
        rel.metadata = Some("x".repeat(MAX_RELATION_METADATA_SIZE));
        assert!(validate_new_relation(&rel).is_ok());
    }
}
