//! Experience management module.
//!
//! An **experience** is the core data type in PulseDB — a unit of learned knowledge
//! stored in a collective. Experiences have content, a semantic embedding for
//! vector search, a rich type, and metadata.
//!
//! # Operations
//!
//! All experience operations are available on [`PulseDB`](crate::PulseDB):
//!
//! - [`record_experience(exp)`](crate::PulseDB::record_experience)
//! - [`get_experience(id)`](crate::PulseDB::get_experience)
//! - [`update_experience(id, update)`](crate::PulseDB::update_experience)
//! - [`archive_experience(id)`](crate::PulseDB::archive_experience)
//! - [`unarchive_experience(id)`](crate::PulseDB::unarchive_experience)
//! - [`delete_experience(id)`](crate::PulseDB::delete_experience)
//! - [`reinforce_experience(id)`](crate::PulseDB::reinforce_experience)

pub mod types;

pub use types::{Experience, ExperienceType, ExperienceUpdate, NewExperience, Severity};

use crate::error::{PulseDBError, ValidationError};
use crate::storage::schema::{
    MAX_CONTENT_SIZE, MAX_DOMAIN_TAGS, MAX_FILE_PATH_LENGTH, MAX_SOURCE_FILES, MAX_TAG_LENGTH,
};

/// Validates a [`NewExperience`] before storage.
///
/// # Rules
///
/// - `content`: non-empty, max 100 KB
/// - `importance`: 0.0–1.0
/// - `confidence`: 0.0–1.0
/// - `domain`: max 10 tags, each max 100 chars
/// - `related_files`: max 10 paths, each max 500 chars
/// - `embedding`: required if `is_external_provider`; dimension must match collective
/// - `source_agent`: non-empty
pub(crate) fn validate_new_experience(
    exp: &NewExperience,
    collective_dimension: u16,
    is_external_provider: bool,
) -> Result<(), PulseDBError> {
    // Content: non-empty
    if exp.content.is_empty() {
        return Err(ValidationError::required_field("content").into());
    }

    // Content: max size
    if exp.content.len() > MAX_CONTENT_SIZE {
        return Err(ValidationError::content_too_large(exp.content.len(), MAX_CONTENT_SIZE).into());
    }

    // Importance: 0.0–1.0
    if !(0.0..=1.0).contains(&exp.importance) {
        return Err(ValidationError::invalid_field(
            "importance",
            format!("must be between 0.0 and 1.0, got {}", exp.importance),
        )
        .into());
    }

    // Confidence: 0.0–1.0
    if !(0.0..=1.0).contains(&exp.confidence) {
        return Err(ValidationError::invalid_field(
            "confidence",
            format!("must be between 0.0 and 1.0, got {}", exp.confidence),
        )
        .into());
    }

    // Domain tags: count limit
    if exp.domain.len() > MAX_DOMAIN_TAGS {
        return Err(
            ValidationError::too_many_items("domain", exp.domain.len(), MAX_DOMAIN_TAGS).into(),
        );
    }

    // Domain tags: individual length limit
    for (i, tag) in exp.domain.iter().enumerate() {
        if tag.len() > MAX_TAG_LENGTH {
            return Err(ValidationError::invalid_field(
                "domain",
                format!(
                    "tag at index {} exceeds max length of {} chars (got {})",
                    i,
                    MAX_TAG_LENGTH,
                    tag.len()
                ),
            )
            .into());
        }
    }

    // Related files: count limit
    if exp.related_files.len() > MAX_SOURCE_FILES {
        return Err(ValidationError::too_many_items(
            "related_files",
            exp.related_files.len(),
            MAX_SOURCE_FILES,
        )
        .into());
    }

    // Related files: individual length limit
    for (i, path) in exp.related_files.iter().enumerate() {
        if path.len() > MAX_FILE_PATH_LENGTH {
            return Err(ValidationError::invalid_field(
                "related_files",
                format!(
                    "path at index {} exceeds max length of {} chars (got {})",
                    i,
                    MAX_FILE_PATH_LENGTH,
                    path.len()
                ),
            )
            .into());
        }
    }

    // Embedding: required for external provider
    if is_external_provider && exp.embedding.is_none() {
        return Err(ValidationError::required_field(
            "embedding (required when using External embedding provider)",
        )
        .into());
    }

    // Embedding: dimension check
    if let Some(ref emb) = exp.embedding {
        if emb.len() != collective_dimension as usize {
            return Err(ValidationError::dimension_mismatch(
                collective_dimension as usize,
                emb.len(),
            )
            .into());
        }
    }

    // Source agent: non-empty
    if exp.source_agent.as_str().is_empty() {
        return Err(ValidationError::required_field("source_agent").into());
    }

    Ok(())
}

/// Validates an [`ExperienceUpdate`] before applying.
///
/// Only validates fields that are `Some(...)`.
pub(crate) fn validate_experience_update(update: &ExperienceUpdate) -> Result<(), PulseDBError> {
    // Importance: 0.0–1.0
    if let Some(importance) = update.importance {
        if !(0.0..=1.0).contains(&importance) {
            return Err(ValidationError::invalid_field(
                "importance",
                format!("must be between 0.0 and 1.0, got {}", importance),
            )
            .into());
        }
    }

    // Confidence: 0.0–1.0
    if let Some(confidence) = update.confidence {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(ValidationError::invalid_field(
                "confidence",
                format!("must be between 0.0 and 1.0, got {}", confidence),
            )
            .into());
        }
    }

    // Domain tags
    if let Some(ref domain) = update.domain {
        if domain.len() > MAX_DOMAIN_TAGS {
            return Err(
                ValidationError::too_many_items("domain", domain.len(), MAX_DOMAIN_TAGS).into(),
            );
        }
        for (i, tag) in domain.iter().enumerate() {
            if tag.len() > MAX_TAG_LENGTH {
                return Err(ValidationError::invalid_field(
                    "domain",
                    format!(
                        "tag at index {} exceeds max length of {} chars (got {})",
                        i,
                        MAX_TAG_LENGTH,
                        tag.len()
                    ),
                )
                .into());
            }
        }
    }

    // Related files
    if let Some(ref files) = update.related_files {
        if files.len() > MAX_SOURCE_FILES {
            return Err(ValidationError::too_many_items(
                "related_files",
                files.len(),
                MAX_SOURCE_FILES,
            )
            .into());
        }
        for (i, path) in files.iter().enumerate() {
            if path.len() > MAX_FILE_PATH_LENGTH {
                return Err(ValidationError::invalid_field(
                    "related_files",
                    format!(
                        "path at index {} exceeds max length of {} chars (got {})",
                        i,
                        MAX_FILE_PATH_LENGTH,
                        path.len()
                    ),
                )
                .into());
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentId, CollectiveId};

    fn valid_new_experience() -> NewExperience {
        NewExperience {
            collective_id: CollectiveId::new(),
            content: "Test experience content".into(),
            experience_type: ExperienceType::default(),
            embedding: Some(vec![0.1; 384]),
            importance: 0.5,
            confidence: 0.5,
            domain: vec!["rust".into()],
            related_files: vec!["src/main.rs".into()],
            source_agent: AgentId::new("agent-1"),
            source_task: None,
        }
    }

    // ====================================================================
    // validate_new_experience tests
    // ====================================================================

    #[test]
    fn test_valid_experience_passes() {
        assert!(validate_new_experience(&valid_new_experience(), 384, true).is_ok());
    }

    #[test]
    fn test_empty_content_rejected() {
        let mut exp = valid_new_experience();
        exp.content = String::new();
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_content_too_large_rejected() {
        let mut exp = valid_new_experience();
        exp.content = "x".repeat(MAX_CONTENT_SIZE + 1);
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_importance_negative_rejected() {
        let mut exp = valid_new_experience();
        exp.importance = -0.1;
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_importance_above_one_rejected() {
        let mut exp = valid_new_experience();
        exp.importance = 1.1;
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_confidence_out_of_range_rejected() {
        let mut exp = valid_new_experience();
        exp.confidence = -0.5;
        assert!(validate_new_experience(&exp, 384, true).is_err());

        exp.confidence = 2.0;
        assert!(validate_new_experience(&exp, 384, true).is_err());
    }

    #[test]
    fn test_too_many_domain_tags_rejected() {
        let mut exp = valid_new_experience();
        exp.domain = (0..51).map(|i| format!("tag-{}", i)).collect();
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_domain_tag_too_long_rejected() {
        let mut exp = valid_new_experience();
        exp.domain = vec!["x".repeat(MAX_TAG_LENGTH + 1)];
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_too_many_related_files_rejected() {
        let mut exp = valid_new_experience();
        exp.related_files = (0..101).map(|i| format!("file-{}.rs", i)).collect();
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_file_path_too_long_rejected() {
        let mut exp = valid_new_experience();
        exp.related_files = vec!["x".repeat(MAX_FILE_PATH_LENGTH + 1)];
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_embedding_required_for_external_provider() {
        let mut exp = valid_new_experience();
        exp.embedding = None;
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_embedding_optional_for_builtin_provider() {
        let mut exp = valid_new_experience();
        exp.embedding = None;
        assert!(validate_new_experience(&exp, 384, false).is_ok());
    }

    #[test]
    fn test_embedding_dimension_mismatch_rejected() {
        let mut exp = valid_new_experience();
        exp.embedding = Some(vec![0.1; 768]); // Expect 384
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    #[test]
    fn test_empty_source_agent_rejected() {
        let mut exp = valid_new_experience();
        exp.source_agent = AgentId::new("");
        let err = validate_new_experience(&exp, 384, true).unwrap_err();
        assert!(err.is_validation());
    }

    // ====================================================================
    // validate_experience_update tests
    // ====================================================================

    #[test]
    fn test_empty_update_passes() {
        assert!(validate_experience_update(&ExperienceUpdate::default()).is_ok());
    }

    #[test]
    fn test_update_valid_importance_passes() {
        let update = ExperienceUpdate {
            importance: Some(0.9),
            ..Default::default()
        };
        assert!(validate_experience_update(&update).is_ok());
    }

    #[test]
    fn test_update_invalid_importance_rejected() {
        let update = ExperienceUpdate {
            importance: Some(1.5),
            ..Default::default()
        };
        assert!(validate_experience_update(&update).is_err());
    }

    #[test]
    fn test_update_invalid_confidence_rejected() {
        let update = ExperienceUpdate {
            confidence: Some(-0.1),
            ..Default::default()
        };
        assert!(validate_experience_update(&update).is_err());
    }

    #[test]
    fn test_update_too_many_domain_tags_rejected() {
        let update = ExperienceUpdate {
            domain: Some((0..51).map(|i| format!("tag-{}", i)).collect()),
            ..Default::default()
        };
        assert!(validate_experience_update(&update).is_err());
    }

    #[test]
    fn test_update_domain_tag_too_long_rejected() {
        let update = ExperienceUpdate {
            domain: Some(vec!["x".repeat(MAX_TAG_LENGTH + 1)]),
            ..Default::default()
        };
        assert!(validate_experience_update(&update).is_err());
    }
}
