//! Type definitions for MCP tool input/output DTOs.
//!
//! Each tool has a dedicated input struct and returns a typed output
//! that is serialized to JSON before transmission over MCP.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Tool input schemas
// ---------------------------------------------------------------------------

/// Input for the `richter_status` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusInput {
    /// Optional: filter to a specific repository id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
}

/// Input for the `richter_repo_status` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatusInput {
    /// The repository id to query.
    pub repo_id: String,
}

/// Input for the `richter_run_or_join` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOrJoinInput {
    /// The command to execute (full argv array).
    pub command: Vec<String>,
    /// The working directory for the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Optional label/tag for the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Whether to wait for the run to complete before returning.
    #[serde(default = "default_wait")]
    pub wait: bool,
}

fn default_wait() -> bool {
    true
}

/// Input for the `richter_active_runs` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveRunsInput {
    /// Optional: filter to a specific repository id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    /// Maximum number of runs to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

/// Input for the `richter_recent_important_events` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentEventsInput {
    /// Maximum number of events to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Optional: filter to a specific repository id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_id: Option<String>,
    /// Optional: minimum importance threshold (0-100).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_importance: Option<u8>,
}

/// Input for the `richter_claim_paths` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimPathsInput {
    /// Paths to claim (relative to workspace root or absolute).
    pub paths: Vec<String>,
    /// Time-to-live for the claim (e.g., "30m", "1h").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
    /// Agent identifier making the claim.
    pub agent_id: String,
}

/// Input for the `richter_release_paths` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleasePathsInput {
    /// Paths to release.
    pub paths: Vec<String>,
    /// Agent identifier releasing the claim.
    pub agent_id: String,
}

/// Input for the `richter_explain_decision` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainDecisionInput {
    /// The decision id to explain.
    pub decision_id: String,
}

/// Input for the `richter_get_run_summary` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRunSummaryInput {
    /// The run id to summarize.
    pub run_id: String,
}

// ---------------------------------------------------------------------------
// Tool output types
// ---------------------------------------------------------------------------

/// Global status output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalStatus {
    pub daemon: DaemonStatus,
    pub active_runs: usize,
    pub queued_runs: usize,
    pub cache_hits_today: u64,
    pub duplicate_work_saved_estimate: String,
    pub system_pressure: SystemPressure,
    pub known_repos: usize,
    pub known_agents: usize,
}

/// Daemon health status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub uptime_seconds: u64,
    pub version: String,
}

/// System pressure snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPressure {
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub disk_percent: f64,
    pub level: String,
}

/// Per-repo status output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    pub repo_id: String,
    pub repo_root: String,
    pub branch: String,
    pub head_sha: String,
    pub dirty: bool,
    pub active_agents: usize,
    pub active_runs: Vec<RunSummary>,
    pub queued_runs: usize,
    pub recent_events: Vec<EventSummary>,
}

/// Run-or-join result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOrJoinResult {
    pub disposition: String,
    pub run_id: String,
    pub message: String,
    pub exit_code: Option<i32>,
    pub cached: bool,
    pub subscriber_count: usize,
}

/// Active run summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub command: Vec<String>,
    pub status: String,
    pub started_at: String,
    pub subscriber_count: usize,
    pub repo_id: Option<String>,
    pub resource_class: String,
}

/// Event summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub event_id: String,
    pub importance: u8,
    pub category: String,
    pub title: String,
    pub summary: String,
    pub occurred_at: String,
    pub repo_id: Option<String>,
    pub run_id: Option<String>,
}

/// Claim result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimResult {
    pub claimed: Vec<String>,
    pub conflicts: Vec<String>,
    pub expires_at: Option<String>,
}

/// Release result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseResult {
    pub released: Vec<String>,
}

/// Decision explanation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionExplanation {
    pub decision_id: String,
    pub command: String,
    pub disposition: String,
    pub reason: String,
    pub fingerprint: String,
    pub decided_at: String,
    pub confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_status_input_defaults() {
        let input: StatusInput = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(input.repo_id.is_none());
    }

    #[test]
    fn deserialize_run_or_join_input_requires_command() {
        let result: Result<RunOrJoinInput, _> = serde_json::from_value(serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_run_or_join_minimal() {
        let input: RunOrJoinInput = serde_json::from_value(serde_json::json!({
            "command": ["echo", "hello"]
        }))
        .unwrap();
        assert_eq!(input.command, vec!["echo", "hello"]);
        assert!(input.wait);
        assert!(input.cwd.is_none());
    }

    #[test]
    fn global_status_json_roundtrip() {
        let status = GlobalStatus {
            daemon: DaemonStatus {
                running: true,
                uptime_seconds: 42,
                version: "0.1.0".to_string(),
            },
            active_runs: 3,
            queued_runs: 1,
            cache_hits_today: 150,
            duplicate_work_saved_estimate: "~12 minutes".to_string(),
            system_pressure: SystemPressure {
                cpu_percent: 45.2,
                memory_percent: 62.1,
                disk_percent: 30.0,
                level: "moderate".to_string(),
            },
            known_repos: 2,
            known_agents: 5,
        };
        let json = serde_json::to_value(&status).unwrap();
        let restored: GlobalStatus = serde_json::from_value(json).unwrap();
        assert_eq!(restored.active_runs, 3);
        assert_eq!(restored.known_agents, 5);
    }

    #[test]
    fn run_summary_roundtrip() {
        let summary = RunSummary {
            run_id: "run-001".to_string(),
            command: vec!["cargo".to_string(), "test".to_string()],
            status: "running".to_string(),
            started_at: "2026-05-04T12:00:00Z".to_string(),
            subscriber_count: 2,
            repo_id: Some("repo-abc".to_string()),
            resource_class: "heavy_test".to_string(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        let restored: RunSummary = serde_json::from_value(json).unwrap();
        assert_eq!(restored.run_id, "run-001");
        assert_eq!(restored.resource_class, "heavy_test");
    }
}
