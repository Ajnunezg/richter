//! Shared data models for the Richter agent-control plane.
//!
//! Defines all core types used across the daemon, CLI, MCP server,
//! and macOS app: repositories, worktrees, agents, runs, events,
//! decisions, leases, and configuration. Every public type derives
//! `Serialize`/`Deserialize` and carries Rustdoc.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Strongly-typed repository identifier.
pub type RepoId = Uuid;

/// Strongly-typed worktree identifier.
pub type WorktreeId = Uuid;

/// Strongly-typed agent identifier.
pub type AgentId = Uuid;

/// Strongly-typed session identifier.
pub type SessionId = Uuid;

/// Strongly-typed run identifier.
pub type RunId = Uuid;

/// Strongly-typed event identifier.
pub type EventId = Uuid;

/// Strongly-typed decision identifier.
pub type DecisionId = Uuid;

/// Strongly-typed lease identifier.
pub type LeaseId = Uuid;

/// Strongly-typed model-call identifier.
pub type ModelCallId = Uuid;

/// Strongly-typed command-invocation identifier.
pub type CommandInvocationId = Uuid;

/// Strongly-typed subscriber identifier.
pub type SubscriberId = Uuid;

/// Strongly-typed artifact identifier.
pub type ArtifactId = Uuid;

/// Strongly-typed cache-entry identifier.
pub type CacheEntryId = Uuid;

/// Strongly-typed important-event identifier.
pub type ImportantEventId = Uuid;

/// Strongly-typed plugin-manifest identifier.
pub type PluginManifestId = Uuid;

/// Strongly-typed setting identifier.
pub type SettingId = Uuid;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Classification of a shell command for scheduling and deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandClass {
    /// Build command (cargo build, make, cmake, etc.).
    Build,
    /// Test command (cargo test, pytest, jest, etc.).
    Test,
    /// Lint command (eslint, ruff, clippy, etc.).
    Lint,
    /// Type-check command (tsc, mypy, etc.).
    Typecheck,
    /// Formatter command (prettier, cargo fmt, etc.).
    Format,
    /// Dependency installation (npm install, pip install, etc.).
    Install,
    /// Dev server or watch mode.
    DevServer,
    /// Database or schema migration.
    Migration,
    /// Potentially destructive command (rm, drop, purge, etc.).
    Destructive,
    /// Unknown / passthrough command.
    Unknown,
}

impl std::fmt::Display for CommandClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CommandClass::Build => "build",
            CommandClass::Test => "test",
            CommandClass::Lint => "lint",
            CommandClass::Typecheck => "typecheck",
            CommandClass::Format => "format",
            CommandClass::Install => "install",
            CommandClass::DevServer => "dev_server",
            CommandClass::Migration => "migration",
            CommandClass::Destructive => "destructive",
            CommandClass::Unknown => "unknown",
        };
        write!(f, "{s}")
    }
}

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

/// Resource class for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    /// Heavy build workload.
    HeavyBuild,
    /// Heavy test workload.
    HeavyTest,
    /// Light lint or type-check workload.
    LightLint,
    /// Dependency installation.
    Install,
    /// Long-running dev server.
    DevServer,
    /// Unknown workload class.
    Unknown,
}

/// The severity of the daemon's global status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonSeverity {
    /// Everything is calm.
    Calm,
    /// There is activity but no problem.
    Active,
    /// Something needs attention.
    Warning,
    /// Immediate action required.
    Critical,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// A Git repository tracked by Richter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    /// Unique identifier.
    pub id: RepoId,
    /// Absolute path to the repository root.
    pub root: PathBuf,
    /// Git common directory (e.g. `.git` or the actual `.git` dir for worktrees).
    pub git_common_dir: PathBuf,
    /// Human-readable name inferred from the directory name.
    pub name: String,
    /// When this repo was first discovered.
    pub discovered_at: DateTime<Utc>,
    /// When this repo was last seen by the watcher.
    pub last_seen_at: DateTime<Utc>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

/// A Git worktree within a repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    /// Unique identifier.
    pub id: WorktreeId,
    /// The parent repository.
    pub repo_id: RepoId,
    /// Absolute path to the worktree root.
    pub path: PathBuf,
    /// The HEAD commit SHA.
    pub head_sha: Option<String>,
    /// The current branch name.
    pub branch: Option<String>,
    /// Whether the worktree is dirty (has uncommitted changes).
    pub is_dirty: bool,
    /// The upstream tracking branch.
    pub upstream: Option<String>,
    /// When this worktree was first discovered.
    pub discovered_at: DateTime<Utc>,
    /// When this worktree was last seen.
    pub last_seen_at: DateTime<Utc>,
}

/// An AI coding agent detected by Richter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    /// Unique identifier.
    pub id: AgentId,
    /// The agent program name (e.g. "claude", "codex", "aider").
    pub agent_type: String,
    /// The process ID of the agent.
    pub pid: Option<u32>,
    /// The working directory of the agent.
    pub cwd: Option<PathBuf>,
    /// The repo this agent is working in.
    pub repo_id: Option<RepoId>,
    /// The worktree this agent is working in.
    pub worktree_id: Option<WorktreeId>,
    /// The current command the agent is running, if any.
    pub active_command: Option<String>,
    /// Files or directories claimed by this agent.
    pub claimed_paths: Vec<PathBuf>,
    /// When the agent was first detected.
    pub first_seen_at: DateTime<Utc>,
    /// When the agent was last seen.
    pub last_seen_at: DateTime<Utc>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

/// A session represents a single invocation of an agent or CLI command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique identifier.
    pub id: SessionId,
    /// The agent that owns this session.
    pub agent_id: AgentId,
    /// When the session started.
    pub started_at: DateTime<Utc>,
    /// When the session ended, if it has.
    pub ended_at: Option<DateTime<Utc>>,
    /// Additional metadata.
    pub metadata: HashMap<String, String>,
}

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

/// A path or file lease claimed by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    /// Unique identifier.
    pub id: LeaseId,
    /// The agent holding the lease.
    pub agent_id: AgentId,
    /// The path being claimed.
    pub path: PathBuf,
    /// When the lease was granted.
    pub granted_at: DateTime<Utc>,
    /// When the lease expires.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the lease is active.
    pub is_active: bool,
    /// The TTL in seconds.
    pub ttl_seconds: Option<i64>,
}

/// A call to an external model (LLM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCall {
    /// Unique identifier.
    pub id: ModelCallId,
    /// The provider name.
    pub provider: String,
    /// The model name.
    pub model: String,
    /// The purpose of the call (classification, summarization, adjudication).
    pub purpose: String,
    /// The input token count.
    pub input_tokens: Option<u64>,
    /// The output token count.
    pub output_tokens: Option<u64>,
    /// The estimated cost in USD.
    pub cost_usd: Option<f64>,
    /// The response latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// When the call was made.
    pub called_at: DateTime<Utc>,
}

/// A key-value setting persisted by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setting {
    /// Unique identifier.
    pub id: SettingId,
    /// The setting key.
    pub key: String,
    /// The setting value (JSON-encoded).
    pub value: serde_json::Value,
    /// When the setting was last updated.
    pub updated_at: DateTime<Utc>,
}

/// A plugin manifest describing an agent integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique identifier.
    pub id: PluginManifestId,
    /// The plugin name.
    pub name: String,
    /// The plugin version.
    pub version: String,
    /// The agent this plugin targets (e.g. "claude", "codex").
    pub agent_type: String,
    /// Whether the plugin is enabled.
    pub enabled: bool,
    /// Plugin configuration (JSON).
    pub config: serde_json::Value,
    /// When the manifest was installed.
    pub installed_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// DTOs for the API and UI
// ---------------------------------------------------------------------------

/// Global status snapshot returned by `richter status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalStatus {
    /// Overall daemon severity.
    pub severity: DaemonSeverity,
    /// Number of tracked repositories.
    pub repo_count: usize,
    /// Number of tracked worktrees.
    pub worktree_count: usize,
    /// Number of detected agents.
    pub agent_count: usize,
    /// Number of active runs.
    pub active_runs: usize,
    /// Number of queued runs.
    pub queued_runs: usize,
    /// Number of cache hits today.
    pub cache_hits_today: u64,
    /// Number of duplicate runs avoided.
    pub duplicates_prevented: u64,
    /// Current CPU usage estimate (0.0-1.0).
    pub cpu_pressure: f64,
    /// Current memory pressure (0.0-1.0).
    pub memory_pressure: f64,
    /// The most important event, if any.
    pub top_event: Option<ImportantEvent>,
    /// Whether daemon coordination is active.
    pub coordination_active: bool,
    /// Whether shims are installed.
    pub shims_installed: bool,
}

/// Repo-level status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatus {
    /// The repository identifier.
    pub repo_id: RepoId,
    /// The repository name.
    pub repo_name: String,
    /// Active agents in this repo.
    pub active_agents: Vec<AgentId>,
    /// Active runs in this repo.
    pub active_runs: Vec<RunId>,
    /// Queued runs in this repo.
    pub queued_runs: Vec<RunId>,
    /// Recent important events.
    pub important_events: Vec<ImportantEvent>,
}

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

/// Resource pressure snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcePressure {
    /// CPU usage (0.0-1.0).
    pub cpu: f64,
    /// Memory usage fraction (0.0-1.0).
    pub memory: f64,
    /// Number of active heavy builds.
    pub active_heavy_builds: usize,
    /// Number of active heavy tests.
    pub active_heavy_tests: usize,
    /// Total active processes under Richter.
    pub total_active_processes: usize,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_class_display() {
        assert_eq!(CommandClass::Build.to_string(), "build");
        assert_eq!(CommandClass::Test.to_string(), "test");
        assert_eq!(CommandClass::Lint.to_string(), "lint");
        assert_eq!(CommandClass::Unknown.to_string(), "unknown");
    }

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
    fn test_event_kind_display() {
        assert_eq!(EventKind::CommandStarted.to_string(), "command_started");
        assert_eq!(EventKind::CacheHit.to_string(), "cache_hit");
    }

    #[test]
    fn test_serialize_command_class() {
        let json = serde_json::to_string(&CommandClass::Build).unwrap();
        assert_eq!(json, "\"build\"");
        let parsed: CommandClass = serde_json::from_str("\"test\"").unwrap();
        assert_eq!(parsed, CommandClass::Test);
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

    #[test]
    fn test_global_status_defaults() {
        let status = GlobalStatus {
            severity: DaemonSeverity::Calm,
            repo_count: 0,
            worktree_count: 0,
            agent_count: 0,
            active_runs: 0,
            queued_runs: 0,
            cache_hits_today: 0,
            duplicates_prevented: 0,
            cpu_pressure: 0.0,
            memory_pressure: 0.0,
            top_event: None,
            coordination_active: true,
            shims_installed: false,
        };
        assert_eq!(status.severity, DaemonSeverity::Calm);
    }

    #[test]
    fn test_resource_pressure_bounds() {
        let rp = ResourcePressure {
            cpu: 0.75,
            memory: 0.60,
            active_heavy_builds: 2,
            active_heavy_tests: 1,
            total_active_processes: 8,
        };
        assert!(rp.cpu >= 0.0 && rp.cpu <= 1.0);
        assert!(rp.memory >= 0.0 && rp.memory <= 1.0);
    }
}
