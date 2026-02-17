//! Data types for experience relations.
//!
//! Relations connect two experiences within the same collective, forming
//! a knowledge graph that agents can traverse to understand how concepts
//! relate to each other.

use serde::{Deserialize, Serialize};

use crate::types::{ExperienceId, RelationId, Timestamp};

/// Type of relationship between two experiences.
///
/// Relations are directed: the semantics describe how the **source**
/// experience relates to the **target** experience.
///
/// # Example
///
/// ```rust
/// use pulsedb::RelationType;
///
/// let rel = RelationType::Supports;
/// // "Experience A supports Experience B"
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelationType {
    /// Source experience supports or reinforces the target.
    Supports,
    /// Source experience contradicts the target.
    Contradicts,
    /// Source experience elaborates on or adds detail to the target.
    Elaborates,
    /// Source experience supersedes or replaces the target.
    Supersedes,
    /// Source experience implies the target.
    Implies,
    /// General relationship with no specific semantics.
    RelatedTo,
}

/// Direction for querying relations from a given experience.
///
/// Relations are directed graphs. When querying, you can ask for:
/// - **Outgoing**: "What does this experience point to?"
/// - **Incoming**: "What points to this experience?"
/// - **Both**: All connections regardless of direction
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationDirection {
    /// Relations where the experience is the source (source → target).
    Outgoing,
    /// Relations where the experience is the target (source → target).
    Incoming,
    /// Both outgoing and incoming relations.
    Both,
}

/// A stored relationship between two experiences.
///
/// Relations are always within the same collective and are directed
/// from `source_id` to `target_id`. The `relation_type` describes
/// the semantic meaning of the connection.
///
/// # Uniqueness
///
/// The combination `(source_id, target_id, relation_type)` must be
/// unique — you cannot have two "Supports" relations between the
/// same pair of experiences.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExperienceRelation {
    /// Unique identifier for this relation.
    pub id: RelationId,

    /// The experience this relation originates from.
    pub source_id: ExperienceId,

    /// The experience this relation points to.
    pub target_id: ExperienceId,

    /// The type of relationship.
    pub relation_type: RelationType,

    /// Strength of the relation (0.0 = weak, 1.0 = strong).
    pub strength: f32,

    /// Optional JSON metadata (max 10KB).
    pub metadata: Option<String>,

    /// When this relation was created.
    pub created_at: Timestamp,
}

/// Input for creating a new relation between two experiences.
///
/// # Example
///
/// ```rust,ignore
/// use pulsedb::{NewExperienceRelation, RelationType};
///
/// let rel = NewExperienceRelation {
///     source_id: exp_a,
///     target_id: exp_b,
///     relation_type: RelationType::Supports,
///     strength: 0.9,
///     metadata: None,
/// };
/// let id = db.store_relation(rel)?;
/// ```
pub struct NewExperienceRelation {
    /// The experience this relation originates from.
    pub source_id: ExperienceId,

    /// The experience this relation points to.
    pub target_id: ExperienceId,

    /// The type of relationship.
    pub relation_type: RelationType,

    /// Strength of the relation (0.0 - 1.0).
    pub strength: f32,

    /// Optional JSON metadata (max 10KB).
    pub metadata: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_relation_type_bincode_roundtrip() {
        let types = [
            RelationType::Supports,
            RelationType::Contradicts,
            RelationType::Elaborates,
            RelationType::Supersedes,
            RelationType::Implies,
            RelationType::RelatedTo,
        ];
        for rt in &types {
            let bytes = bincode::serialize(rt).unwrap();
            let restored: RelationType = bincode::deserialize(&bytes).unwrap();
            assert_eq!(*rt, restored);
        }
    }

    #[test]
    fn test_experience_relation_bincode_roundtrip() {
        let relation = ExperienceRelation {
            id: RelationId::new(),
            source_id: ExperienceId::new(),
            target_id: ExperienceId::new(),
            relation_type: RelationType::Supports,
            strength: 0.85,
            metadata: Some(r#"{"context": "test"}"#.to_string()),
            created_at: Timestamp::now(),
        };

        let bytes = bincode::serialize(&relation).unwrap();
        let restored: ExperienceRelation = bincode::deserialize(&bytes).unwrap();

        assert_eq!(relation.id, restored.id);
        assert_eq!(relation.source_id, restored.source_id);
        assert_eq!(relation.target_id, restored.target_id);
        assert_eq!(relation.relation_type, restored.relation_type);
        assert_eq!(relation.strength, restored.strength);
        assert_eq!(relation.metadata, restored.metadata);
    }

    #[test]
    fn test_relation_type_copy_and_eq() {
        let a = RelationType::Contradicts;
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn test_relation_direction_variants() {
        // Ensure all 3 variants are distinct
        assert_ne!(RelationDirection::Outgoing, RelationDirection::Incoming);
        assert_ne!(RelationDirection::Outgoing, RelationDirection::Both);
        assert_ne!(RelationDirection::Incoming, RelationDirection::Both);
    }
}
