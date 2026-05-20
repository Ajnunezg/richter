//! Run-or-join execution model: runs, subscribers, artifacts, cache entries,
//! invocations, decisions, and their lifecycle statuses.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::command::CommandClass;
use super::ids::{
    AgentId, ArtifactId, CacheEntryId, CommandInvocationId, DecisionId, ModelCallId, RepoId, RunId,
    SessionId, SubscriberId, WorktreeId,
};

// ---------------------------------------------------------------------------
// RunStatus
// ---------------------------------------------------------------------------

/// The lifecycle status of a tracked run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The run has been accepted and is waiting for resources.
    Queued,
    /// The command is currently executing.
    Running,
    /// The command completed successfully (exit code 0).
    Passed,
    /// The command failed (non-zero exit code).
    Failed,
    /// The run was cancelled by the user or policy.
    Cancelled,
    /// The run timed out.
    TimedOut,
    /// A result was served from the cache.
    Cached,
    /// The subscriber joined an already-running equivalent run.
    Joined,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RunStatus::Queued => "queued",
            RunStatus::Running => "running",
            RunStatus::Passed => "passed",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::TimedOut => "timed_out",
            RunStatus::Cached => "cached",
            RunStatus::Joined => "joined",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for RunStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(RunStatus::Queued),
            "running" => Ok(RunStatus::Running),
            "passed" => Ok(RunStatus::Passed),
            "failed" => Ok(RunStatus::Failed),
            "cancelled" => Ok(RunStatus::Cancelled),
            "timed_out" => Ok(RunStatus::TimedOut),
            "cached" => Ok(RunStatus::Cached),
            "joined" => Ok(RunStatus::Joined),
            other => Err(format!("unknown RunStatus: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// DecisionOutcome
// ---------------------------------------------------------------------------

/// Decision outcome from the run-or-join engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionOutcome {
    /// Start a new run.
    Run,
    /// Join an existing equivalent run.
    Join,
    /// Return a fresh cached result.
    Cache,
    /// Queue the command until resources are freed.
    Queue,
    /// Block the command (e.g. destructive without policy).
    Block,
    /// Pass through without management.
    Passthrough,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A raw command invocation as received by Richter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandInvocation {
    /// Unique identifier.
    pub id: CommandInvocationId,
    /// The session that invoked the command.
    pub session_id: SessionId,
    /// The agent that invoked the command.
    pub agent_id: AgentId,
    /// The repo this command targets.
    pub repo_id: Option<RepoId>,
    /// The worktree this command targets.
    pub worktree_id: Option<WorktreeId>,
    /// The full argument vector.
    pub argv: Vec<String>,
    /// The working directory when invoked.
    pub cwd: PathBuf,
    /// The classified command class.
    pub command_class: CommandClass,
    /// The computed fingerprint.
    pub fingerprint: String,
    /// Environment variables (redacted).
    pub env: HashMap<String, String>,
    /// When the invocation was received.
    pub received_at: DateTime<Utc>,
}

/// A run represents the execution (or attempted execution) of a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    /// Unique identifier.
    pub id: RunId,
    /// The command invocation that triggered this run.
    pub invocation_id: CommandInvocationId,
    /// The repo this run belongs to.
    pub repo_id: Option<RepoId>,
    /// The worktree this run belongs to.
    pub worktree_id: Option<WorktreeId>,
    /// The classified command class.
    pub command_class: CommandClass,
    /// The computed fingerprint.
    pub fingerprint: String,
    /// Current status.
    pub status: RunStatus,
    /// The leader run ID if this is a joined run.
    pub leader_run_id: Option<RunId>,
    /// Exit code, if completed.
    pub exit_code: Option<i32>,
    /// Path to stdout log.
    pub stdout_log_path: Option<PathBuf>,
    /// Path to stderr log.
    pub stderr_log_path: Option<PathBuf>,
    /// When the run was created.
    pub created_at: DateTime<Utc>,
    /// When the run started executing.
    pub started_at: Option<DateTime<Utc>>,
    /// When the run finished.
    pub finished_at: Option<DateTime<Utc>>,
    /// Duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

/// A subscriber to a run (agent or session watching the output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSubscriber {
    /// Unique identifier.
    pub id: SubscriberId,
    /// The run being subscribed to.
    pub run_id: RunId,
    /// The agent subscribing.
    pub agent_id: AgentId,
    /// The session subscribing.
    pub session_id: SessionId,
    /// When the subscription started.
    pub subscribed_at: DateTime<Utc>,
    /// When the subscription ended.
    pub unsubscribed_at: Option<DateTime<Utc>>,
}

/// An artifact produced by a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    /// Unique identifier.
    pub id: ArtifactId,
    /// The run that produced this artifact.
    pub run_id: RunId,
    /// Human-readable name.
    pub name: String,
    /// Path to the artifact on disk.
    pub path: PathBuf,
    /// MIME type if known.
    pub mime_type: Option<String>,
    /// Size in bytes.
    pub size_bytes: Option<u64>,
    /// SHA-256 hash of the artifact.
    pub sha256: Option<String>,
    /// When the artifact was created.
    pub created_at: DateTime<Utc>,
}

/// A cache entry for a previously executed command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Unique identifier.
    pub id: CacheEntryId,
    /// The run that produced this cached result.
    pub run_id: RunId,
    /// The fingerprint this cache entry matches.
    pub fingerprint: String,
    /// The command class.
    pub command_class: CommandClass,
    /// The exit code.
    pub exit_code: i32,
    /// Path to stdout log.
    pub stdout_log_path: Option<PathBuf>,
    /// Path to stderr log.
    pub stderr_log_path: Option<PathBuf>,
    /// When this entry was created.
    pub created_at: DateTime<Utc>,
    /// When this entry expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// The repo this cache entry belongs to.
    pub repo_id: Option<RepoId>,
    /// The worktree this cache entry belongs to.
    pub worktree_id: Option<WorktreeId>,
}

/// A run-or-join decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// Unique identifier.
    pub id: DecisionId,
    /// The command invocation that prompted this decision.
    pub invocation_id: CommandInvocationId,
    /// The decision outcome.
    pub outcome: DecisionOutcome,
    /// The run ID if joining or returning from cache.
    pub target_run_id: Option<RunId>,
    /// The reason for this decision.
    pub reason: String,
    /// Whether an LLM was consulted.
    pub llm_consulted: bool,
    /// The model call ID if an LLM was consulted.
    pub model_call_id: Option<ModelCallId>,
    /// When the decision was made.
    pub decided_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Result of a run-or-join request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOrJoinResult {
    /// The decision that was made.
    pub outcome: DecisionOutcome,
    /// The run ID (new or joined).
    pub run_id: RunId,
    /// Human-readable message.
    pub message: String,
    /// Whether the result was cached.
    pub cached: bool,
    /// Exit code if available immediately.
    pub exit_code: Option<i32>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_status_display() {
        assert_eq!(RunStatus::Queued.to_string(), "queued");
        assert_eq!(RunStatus::Running.to_string(), "running");
        assert_eq!(RunStatus::Passed.to_string(), "passed");
        assert_eq!(RunStatus::Failed.to_string(), "failed");
        assert_eq!(RunStatus::Cached.to_string(), "cached");
        assert_eq!(RunStatus::Joined.to_string(), "joined");
    }

    #[test]
    fn test_serialize_run_status() {
        let json = serde_json::to_string(&RunStatus::Cached).unwrap();
        assert_eq!(json, "\"cached\"");
    }

    #[test]
    fn test_serialize_decision_outcome() {
        let json = serde_json::to_string(&DecisionOutcome::Join).unwrap();
        assert_eq!(json, "\"join\"");
    }
}
