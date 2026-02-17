//! Data types for derived insights.
//!
//! Insights are synthesized knowledge computed from multiple experiences
//! within the same collective. They represent higher-level understanding
//! that agents can use for decision-making.

use serde::{Deserialize, Serialize};

use crate::types::{CollectiveId, ExperienceId, InsightId, Timestamp};

/// Type of derived insight.
///
/// Categorizes the kind of synthesis that produced this insight,
/// enabling agents to filter and prioritize different knowledge types.
///
/// # Example
///
/// ```rust
/// use pulsedb::InsightType;
///
/// let kind = InsightType::Pattern;
/// // "A recurring pattern detected across experiences"
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InsightType {
    /// A recurring pattern detected across multiple experiences.
    Pattern,
    /// A synthesis combining knowledge from multiple experiences.
    Synthesis,
    /// An abstraction generalizing multiple specific experiences.
    Abstraction,
    /// A correlation detected between experiences.
    Correlation,
}

/// A stored derived insight — synthesized knowledge from multiple experiences.
///
/// Unlike experiences (which are raw agent observations), insights are
/// computed/derived knowledge that represents higher-level understanding.
///
/// # Embedding Storage
///
/// Insight embeddings are stored **inline** (not in a separate table) because:
/// 1. Insights are expected to be far fewer than experiences
/// 2. The insight record is always loaded with its embedding for HNSW rebuild
/// 3. Simpler storage model (no table join needed)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DerivedInsight {
    /// Unique identifier (UUID v7, time-ordered).
    pub id: InsightId,

    /// The collective this insight belongs to.
    pub collective_id: CollectiveId,

    /// The insight content (text).
    pub content: String,

    /// Semantic embedding vector for similarity search.
    pub embedding: Vec<f32>,

    /// IDs of the source experiences this insight was derived from.
    pub source_experience_ids: Vec<ExperienceId>,

    /// The type of derivation.
    pub insight_type: InsightType,

    /// Confidence in this insight (0.0 = uncertain, 1.0 = certain).
    pub confidence: f32,

    /// Domain tags for categorical filtering.
    pub domain: Vec<String>,

    /// When this insight was created.
    pub created_at: Timestamp,

    /// When this insight was last updated.
    pub updated_at: Timestamp,
}

/// Input for creating a new derived insight.
///
/// The `embedding` field is required when using the External embedding
/// provider, and optional when using the Builtin provider (which
/// generates embeddings from content automatically).
///
/// # Example
///
/// ```rust,ignore
/// use pulsedb::{NewDerivedInsight, InsightType};
///
/// let insight = NewDerivedInsight {
///     collective_id,
///     content: "Error handling patterns converge on early return".to_string(),
///     embedding: Some(embedding_vec),
///     source_experience_ids: vec![exp_a, exp_b, exp_c],
///     insight_type: InsightType::Pattern,
///     confidence: 0.85,
///     domain: vec!["rust".to_string(), "error-handling".to_string()],
/// };
/// let id = db.store_insight(insight)?;
/// ```
pub struct NewDerivedInsight {
    /// The collective to store this insight in.
    pub collective_id: CollectiveId,

    /// The insight content (text, max 50KB).
    pub content: String,

    /// Pre-computed embedding vector (required for External provider).
    pub embedding: Option<Vec<f32>>,

    /// IDs of the source experiences this insight was derived from (1-100).
    pub source_experience_ids: Vec<ExperienceId>,

    /// The type of derivation.
    pub insight_type: InsightType,

    /// Confidence in this insight (0.0-1.0).
    pub confidence: f32,

    /// Domain tags for categorical filtering.
    pub domain: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insight_type_bincode_roundtrip() {
        let types = [
            InsightType::Pattern,
            InsightType::Synthesis,
            InsightType::Abstraction,
            InsightType::Correlation,
        ];
        for it in &types {
            let bytes = bincode::serialize(it).unwrap();
            let restored: InsightType = bincode::deserialize(&bytes).unwrap();
            assert_eq!(*it, restored);
        }
    }

    #[test]
    fn test_derived_insight_bincode_roundtrip() {
        let insight = DerivedInsight {
            id: InsightId::new(),
            collective_id: CollectiveId::new(),
            content: "Test insight content".to_string(),
            embedding: vec![0.1, 0.2, 0.3],
            source_experience_ids: vec![ExperienceId::new(), ExperienceId::new()],
            insight_type: InsightType::Pattern,
            confidence: 0.85,
            domain: vec!["rust".to_string()],
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        };

        let bytes = bincode::serialize(&insight).unwrap();
        let restored: DerivedInsight = bincode::deserialize(&bytes).unwrap();

        assert_eq!(insight.id, restored.id);
        assert_eq!(insight.collective_id, restored.collective_id);
        assert_eq!(insight.content, restored.content);
        assert_eq!(insight.embedding, restored.embedding);
        assert_eq!(
            insight.source_experience_ids,
            restored.source_experience_ids
        );
        assert_eq!(insight.insight_type, restored.insight_type);
        assert_eq!(insight.confidence, restored.confidence);
        assert_eq!(insight.domain, restored.domain);
    }

    #[test]
    fn test_insight_type_copy_and_eq() {
        let a = InsightType::Synthesis;
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn test_insight_type_all_variants_distinct() {
        assert_ne!(InsightType::Pattern, InsightType::Synthesis);
        assert_ne!(InsightType::Synthesis, InsightType::Abstraction);
        assert_ne!(InsightType::Abstraction, InsightType::Correlation);
        assert_ne!(InsightType::Pattern, InsightType::Correlation);
    }
}
