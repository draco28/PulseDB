//! Insight management module.
//!
//! A **derived insight** is synthesized knowledge computed from multiple
//! experiences within the same collective. Insights represent higher-level
//! understanding that agents can use for decision-making.
//!
//! # Operations
//!
//! All insight operations are available on [`PulseDB`](crate::PulseDB):
//!
//! - [`store_insight(insight)`](crate::PulseDB::store_insight)
//! - [`get_insight(id)`](crate::PulseDB::get_insight)
//! - [`get_insights(collective_id, query, k)`](crate::PulseDB::get_insights)
//! - [`delete_insight(id)`](crate::PulseDB::delete_insight)
//!
//! # Constraints
//!
//! - Content must be non-empty and ≤ 50KB
//! - Confidence must be in `[0.0, 1.0]`
//! - At least 1 and at most 100 source experience IDs
//! - All source experiences must belong to the same collective

pub mod types;

pub use types::{DerivedInsight, InsightType, NewDerivedInsight};

use crate::error::{PulseDBError, ValidationError};
use crate::storage::schema::{MAX_INSIGHT_CONTENT_SIZE, MAX_INSIGHT_SOURCES};

/// Validates a new insight before storage.
///
/// Checks:
/// - Content is non-empty and within 50KB
/// - Confidence is in the valid range [0.0, 1.0]
/// - At least one source experience ID is provided
/// - No more than 100 source experience IDs
///
/// Does NOT check cross-collective or existence constraints — those
/// require storage lookups and are handled by the PulseDB facade.
pub(crate) fn validate_new_insight(insight: &NewDerivedInsight) -> Result<(), PulseDBError> {
    // Content must be non-empty
    if insight.content.is_empty() {
        return Err(ValidationError::required_field("content").into());
    }

    // Content size limit
    if insight.content.len() > MAX_INSIGHT_CONTENT_SIZE {
        return Err(ValidationError::content_too_large(
            insight.content.len(),
            MAX_INSIGHT_CONTENT_SIZE,
        )
        .into());
    }

    // Confidence range
    if !(0.0..=1.0).contains(&insight.confidence) {
        return Err(ValidationError::invalid_field(
            "confidence",
            format!("must be between 0.0 and 1.0, got {}", insight.confidence),
        )
        .into());
    }

    // Must have at least one source
    if insight.source_experience_ids.is_empty() {
        return Err(ValidationError::required_field("source_experience_ids").into());
    }

    // Source count limit
    if insight.source_experience_ids.len() > MAX_INSIGHT_SOURCES {
        return Err(ValidationError::too_many_items(
            "source_experience_ids",
            insight.source_experience_ids.len(),
            MAX_INSIGHT_SOURCES,
        )
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CollectiveId, ExperienceId};

    fn valid_new_insight() -> NewDerivedInsight {
        NewDerivedInsight {
            collective_id: CollectiveId::new(),
            content: "Error handling patterns converge on early return".to_string(),
            embedding: None,
            source_experience_ids: vec![ExperienceId::new(), ExperienceId::new()],
            insight_type: InsightType::Pattern,
            confidence: 0.85,
            domain: vec!["rust".to_string()],
        }
    }

    #[test]
    fn test_valid_insight_passes() {
        let insight = valid_new_insight();
        assert!(validate_new_insight(&insight).is_ok());
    }

    #[test]
    fn test_empty_content_rejected() {
        let mut insight = valid_new_insight();
        insight.content = String::new();
        let err = validate_new_insight(&insight).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("content"));
    }

    #[test]
    fn test_content_too_large_rejected() {
        let mut insight = valid_new_insight();
        insight.content = "x".repeat(MAX_INSIGHT_CONTENT_SIZE + 1);
        let err = validate_new_insight(&insight).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn test_confidence_below_zero_rejected() {
        let mut insight = valid_new_insight();
        insight.confidence = -0.1;
        let err = validate_new_insight(&insight).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("confidence"));
    }

    #[test]
    fn test_confidence_above_one_rejected() {
        let mut insight = valid_new_insight();
        insight.confidence = 1.1;
        let err = validate_new_insight(&insight).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("confidence"));
    }

    #[test]
    fn test_confidence_boundary_values() {
        let mut insight = valid_new_insight();

        insight.confidence = 0.0;
        assert!(validate_new_insight(&insight).is_ok());

        insight.confidence = 1.0;
        assert!(validate_new_insight(&insight).is_ok());
    }

    #[test]
    fn test_empty_sources_rejected() {
        let mut insight = valid_new_insight();
        insight.source_experience_ids = vec![];
        let err = validate_new_insight(&insight).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("source_experience_ids"));
    }

    #[test]
    fn test_too_many_sources_rejected() {
        let mut insight = valid_new_insight();
        insight.source_experience_ids = (0..=MAX_INSIGHT_SOURCES)
            .map(|_| ExperienceId::new())
            .collect();
        let err = validate_new_insight(&insight).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("Too many"));
    }

    #[test]
    fn test_sources_at_limit_passes() {
        let mut insight = valid_new_insight();
        insight.source_experience_ids = (0..MAX_INSIGHT_SOURCES)
            .map(|_| ExperienceId::new())
            .collect();
        assert!(validate_new_insight(&insight).is_ok());
    }
}
