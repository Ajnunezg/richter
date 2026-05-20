//! Event and importance-pipeline types emitted by the Richter daemon.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::ids::{AgentId, EventId, ImportantEventId, RepoId, RunId};

// ---------------------------------------------------------------------------
// EventKind
// ---------------------------------------------------------------------------

/// The kind of event emitted by the Richter daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// A command started executing.
    CommandStarted,
    /// A command joined an existing run.
    CommandJoined,
    /// A command was queued.
    CommandQueued,
    /// A cache hit was returned.
    CacheHit,
    /// A command failed.
    CommandFailed,
    /// A command passed.
    CommandPassed,
    /// First test failure in a run.
    FirstTestFailure,
    /// A repeated flaky failure was detected.
    RepeatedFlakyFailure,
    /// A build error occurred.
    BuildError,
    /// A type error occurred.
    TypeError,
    /// Linter output summary.
    LinterSummary,
    /// Resource pressure detected.
    ResourcePressure,
    /// Two agents have a conflict (e.g. same file).
    AgentConflict,
    /// A file/path lease conflict.
    LeaseConflict,
    /// A command has been running with no progress for too long.
    NoProgress,
    /// Repo dirty state changed.
    DirtyStateChange,
    /// Dependency install changed.
    DependencyInstallChange,
    /// A potentially destructive command was detected.
    PotentialDestructive,
    /// A generic informational event.
    Info,
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            EventKind::CommandStarted => "command_started",
            EventKind::CommandJoined => "command_joined",
            EventKind::CommandQueued => "command_queued",
            EventKind::CacheHit => "cache_hit",
            EventKind::CommandFailed => "command_failed",
            EventKind::CommandPassed => "command_passed",
            EventKind::FirstTestFailure => "first_test_failure",
            EventKind::RepeatedFlakyFailure => "repeated_flaky_failure",
            EventKind::BuildError => "build_error",
            EventKind::TypeError => "type_error",
            EventKind::LinterSummary => "linter_summary",
            EventKind::ResourcePressure => "resource_pressure",
            EventKind::AgentConflict => "agent_conflict",
            EventKind::LeaseConflict => "lease_conflict",
            EventKind::NoProgress => "no_progress",
            EventKind::DirtyStateChange => "dirty_state_change",
            EventKind::DependencyInstallChange => "dependency_install_change",
            EventKind::PotentialDestructive => "potential_destructive",
            EventKind::Info => "info",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for EventKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "command_started" => Ok(EventKind::CommandStarted),
            "command_joined" => Ok(EventKind::CommandJoined),
            "command_queued" => Ok(EventKind::CommandQueued),
            "cache_hit" => Ok(EventKind::CacheHit),
            "command_failed" => Ok(EventKind::CommandFailed),
            "command_passed" => Ok(EventKind::CommandPassed),
            "first_test_failure" => Ok(EventKind::FirstTestFailure),
            "repeated_flaky_failure" => Ok(EventKind::RepeatedFlakyFailure),
            "build_error" => Ok(EventKind::BuildError),
            "type_error" => Ok(EventKind::TypeError),
            "linter_summary" => Ok(EventKind::LinterSummary),
            "resource_pressure" => Ok(EventKind::ResourcePressure),
            "agent_conflict" => Ok(EventKind::AgentConflict),
            "lease_conflict" => Ok(EventKind::LeaseConflict),
            "no_progress" => Ok(EventKind::NoProgress),
            "dirty_state_change" => Ok(EventKind::DirtyStateChange),
            "dependency_install_change" => Ok(EventKind::DependencyInstallChange),
            "potential_destructive" => Ok(EventKind::PotentialDestructive),
            "info" => Ok(EventKind::Info),
            other => Err(format!("unknown EventKind: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// ImportanceLevel
// ---------------------------------------------------------------------------

/// Importance level for events shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportanceLevel {
    /// Low-priority event, kept in logs but not surfaced.
    Low,
    /// Medium-priority event, surfaced in dashboard but not notified.
    Medium,
    /// High-priority event, surfaced and pushed as macOS notification.
    High,
    /// Critical event requiring immediate attention.
    Critical,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A generic event emitted by the Richter daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique identifier.
    pub id: EventId,
    /// The kind of event.
    pub kind: EventKind,
    /// The run this event relates to, if any.
    pub run_id: Option<RunId>,
    /// The agent this event relates to, if any.
    pub agent_id: Option<AgentId>,
    /// The repo this event relates to, if any.
    pub repo_id: Option<RepoId>,
    /// Human-readable title.
    pub title: String,
    /// Human-readable description.
    pub description: Option<String>,
    /// Structured payload (JSON).
    pub payload: Option<serde_json::Value>,
    /// When the event was emitted.
    pub emitted_at: DateTime<Utc>,
}

/// An important event that passed the importance pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportantEvent {
    /// Unique identifier.
    pub id: ImportantEventId,
    /// The underlying raw event.
    pub event_id: EventId,
    /// Computed importance score (0-100).
    pub importance: u8,
    /// Importance category.
    pub level: ImportanceLevel,
    /// The category label.
    pub category: String,
    /// Concise title for notification.
    pub title: String,
    /// Summary for display.
    pub summary: String,
    /// Whether this should trigger a macOS notification.
    pub should_notify_user: bool,
    /// Whether this should be surfaced to agents.
    pub should_surface_to_agents: bool,
    /// Recommended action, if any.
    pub recommended_action: Option<String>,
    /// Confidence in the importance classification (0.0 - 1.0).
    pub confidence: f64,
    /// When this was emitted.
    pub emitted_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_kind_display() {
        assert_eq!(EventKind::CommandStarted.to_string(), "command_started");
        assert_eq!(EventKind::CacheHit.to_string(), "cache_hit");
    }
}
