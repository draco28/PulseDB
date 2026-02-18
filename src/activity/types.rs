//! Data types for agent activity tracking.
//!
//! Activities represent the presence and current state of an agent
//! within a collective. Unlike other PulseDB entities, activities are
//! keyed by `(collective_id, agent_id)` composite rather than a UUID,
//! since each agent can have at most one active session per collective.

use serde::{Deserialize, Serialize};

use crate::types::{CollectiveId, Timestamp};

/// A stored agent activity — presence record within a collective.
///
/// Activities track which agents are currently operating in a collective,
/// what they're working on, and when they last checked in. This enables
/// agent coordination and discovery.
///
/// # Key Design
///
/// Activities are uniquely identified by `(collective_id, agent_id)`.
/// Re-registering with the same pair replaces the existing activity
/// (upsert semantics).
///
/// # Staleness
///
/// An activity is considered stale when `last_heartbeat` is older than
/// `Config::activity::stale_threshold`. Stale activities are excluded
/// from `get_active_agents()` results but remain in storage until
/// explicitly ended or the collective is deleted.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Activity {
    /// The agent's identifier (e.g., "claude-opus", "agent-47").
    pub agent_id: String,

    /// The collective this activity belongs to.
    pub collective_id: CollectiveId,

    /// What the agent is currently working on (max 1KB).
    pub current_task: Option<String>,

    /// Summary of the agent's current context (max 1KB).
    pub context_summary: Option<String>,

    /// When this activity was first registered.
    pub started_at: Timestamp,

    /// When the agent last sent a heartbeat.
    pub last_heartbeat: Timestamp,
}

/// Input for registering a new agent activity.
///
/// The `started_at` and `last_heartbeat` timestamps are set automatically
/// to `Timestamp::now()` when the activity is registered.
///
/// # Example
///
/// ```rust,ignore
/// use pulsedb::NewActivity;
///
/// let activity = NewActivity {
///     agent_id: "claude-opus".to_string(),
///     collective_id,
///     current_task: Some("Implementing error handling".to_string()),
///     context_summary: Some("Working on src/error.rs".to_string()),
/// };
/// db.register_activity(activity)?;
/// ```
pub struct NewActivity {
    /// The agent's identifier (non-empty, max 255 bytes).
    pub agent_id: String,

    /// The collective to register in (must exist).
    pub collective_id: CollectiveId,

    /// What the agent is currently working on (max 1KB).
    pub current_task: Option<String>,

    /// Summary of the agent's current context (max 1KB).
    pub context_summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activity_bincode_roundtrip() {
        let activity = Activity {
            agent_id: "claude-opus".to_string(),
            collective_id: CollectiveId::new(),
            current_task: Some("Implementing feature X".to_string()),
            context_summary: Some("Working on module Y".to_string()),
            started_at: Timestamp::now(),
            last_heartbeat: Timestamp::now(),
        };

        let bytes = bincode::serialize(&activity).unwrap();
        let restored: Activity = bincode::deserialize(&bytes).unwrap();

        assert_eq!(activity.agent_id, restored.agent_id);
        assert_eq!(activity.collective_id, restored.collective_id);
        assert_eq!(activity.current_task, restored.current_task);
        assert_eq!(activity.context_summary, restored.context_summary);
        assert_eq!(activity.started_at, restored.started_at);
        assert_eq!(activity.last_heartbeat, restored.last_heartbeat);
    }

    #[test]
    fn test_activity_with_optional_fields_roundtrip() {
        let activity = Activity {
            agent_id: "agent-minimal".to_string(),
            collective_id: CollectiveId::new(),
            current_task: None,
            context_summary: None,
            started_at: Timestamp::from_millis(1000),
            last_heartbeat: Timestamp::from_millis(2000),
        };

        let bytes = bincode::serialize(&activity).unwrap();
        let restored: Activity = bincode::deserialize(&bytes).unwrap();

        assert_eq!(activity.agent_id, restored.agent_id);
        assert!(restored.current_task.is_none());
        assert!(restored.context_summary.is_none());
        assert_eq!(activity.started_at, restored.started_at);
        assert_eq!(activity.last_heartbeat, restored.last_heartbeat);
    }
}
