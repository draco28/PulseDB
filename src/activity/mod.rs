//! Agent activity tracking module.
//!
//! An **activity** represents the presence and current state of an agent
//! within a collective. Activities enable agent coordination and discovery.
//!
//! # Operations
//!
//! All activity operations are available on [`PulseDB`](crate::PulseDB):
//!
//! - [`register_activity(activity)`](crate::PulseDB::register_activity)
//! - [`update_heartbeat(agent_id, collective_id)`](crate::PulseDB::update_heartbeat)
//! - [`end_activity(agent_id, collective_id)`](crate::PulseDB::end_activity)
//! - [`get_active_agents(collective_id)`](crate::PulseDB::get_active_agents)
//!
//! # Constraints
//!
//! - Agent ID must be non-empty and ≤ 255 bytes
//! - `current_task` and `context_summary` must each be ≤ 1KB
//! - One activity per `(collective_id, agent_id)` pair (upsert semantics)

pub mod types;

pub use types::{Activity, NewActivity};

use crate::error::{PulseDBError, ValidationError};
use crate::storage::schema::{MAX_ACTIVITY_AGENT_ID_LENGTH, MAX_ACTIVITY_FIELD_SIZE};

/// Validates a new activity before storage.
///
/// Checks:
/// - Agent ID is non-empty
/// - Agent ID doesn't exceed 255 bytes
/// - `current_task` (if provided) doesn't exceed 1KB
/// - `context_summary` (if provided) doesn't exceed 1KB
///
/// Does NOT check collective existence — that requires a storage lookup
/// and is handled by the PulseDB facade.
pub(crate) fn validate_new_activity(activity: &NewActivity) -> Result<(), PulseDBError> {
    // Agent ID must be non-empty
    if activity.agent_id.is_empty() {
        return Err(ValidationError::required_field("agent_id").into());
    }

    // Agent ID length limit
    if activity.agent_id.len() > MAX_ACTIVITY_AGENT_ID_LENGTH {
        return Err(ValidationError::invalid_field(
            "agent_id",
            format!(
                "must be at most {} bytes, got {}",
                MAX_ACTIVITY_AGENT_ID_LENGTH,
                activity.agent_id.len()
            ),
        )
        .into());
    }

    // current_task size limit
    if let Some(ref task) = activity.current_task {
        if task.len() > MAX_ACTIVITY_FIELD_SIZE {
            return Err(
                ValidationError::content_too_large(task.len(), MAX_ACTIVITY_FIELD_SIZE).into(),
            );
        }
    }

    // context_summary size limit
    if let Some(ref summary) = activity.context_summary {
        if summary.len() > MAX_ACTIVITY_FIELD_SIZE {
            return Err(
                ValidationError::content_too_large(summary.len(), MAX_ACTIVITY_FIELD_SIZE).into(),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CollectiveId;

    fn valid_new_activity() -> NewActivity {
        NewActivity {
            agent_id: "claude-opus".to_string(),
            collective_id: CollectiveId::new(),
            current_task: Some("Implementing feature X".to_string()),
            context_summary: Some("Working on module Y".to_string()),
        }
    }

    #[test]
    fn test_valid_activity_passes() {
        let activity = valid_new_activity();
        assert!(validate_new_activity(&activity).is_ok());
    }

    #[test]
    fn test_valid_activity_no_optional_fields() {
        let activity = NewActivity {
            agent_id: "agent-1".to_string(),
            collective_id: CollectiveId::new(),
            current_task: None,
            context_summary: None,
        };
        assert!(validate_new_activity(&activity).is_ok());
    }

    #[test]
    fn test_empty_agent_id_rejected() {
        let mut activity = valid_new_activity();
        activity.agent_id = String::new();
        let err = validate_new_activity(&activity).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("agent_id"));
    }

    #[test]
    fn test_agent_id_too_long_rejected() {
        let mut activity = valid_new_activity();
        activity.agent_id = "x".repeat(MAX_ACTIVITY_AGENT_ID_LENGTH + 1);
        let err = validate_new_activity(&activity).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("agent_id"));
    }

    #[test]
    fn test_agent_id_at_limit_passes() {
        let mut activity = valid_new_activity();
        activity.agent_id = "x".repeat(MAX_ACTIVITY_AGENT_ID_LENGTH);
        assert!(validate_new_activity(&activity).is_ok());
    }

    #[test]
    fn test_current_task_too_large_rejected() {
        let mut activity = valid_new_activity();
        activity.current_task = Some("x".repeat(MAX_ACTIVITY_FIELD_SIZE + 1));
        let err = validate_new_activity(&activity).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn test_context_summary_too_large_rejected() {
        let mut activity = valid_new_activity();
        activity.context_summary = Some("x".repeat(MAX_ACTIVITY_FIELD_SIZE + 1));
        let err = validate_new_activity(&activity).unwrap_err();
        assert!(err.is_validation());
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn test_fields_at_limit_passes() {
        let mut activity = valid_new_activity();
        activity.current_task = Some("x".repeat(MAX_ACTIVITY_FIELD_SIZE));
        activity.context_summary = Some("y".repeat(MAX_ACTIVITY_FIELD_SIZE));
        assert!(validate_new_activity(&activity).is_ok());
    }
}
