//! Agent, session, lease, and plugin-integration types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::ids::{AgentId, LeaseId, PluginManifestId, RepoId, SessionId, WorktreeId};

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Lease
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// PluginManifest
// ---------------------------------------------------------------------------

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
