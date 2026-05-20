//! Repository and worktree types for Git workspace tracking.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::event::ImportantEvent;
use super::ids::{AgentId, RepoId, RunId, WorktreeId};

// ---------------------------------------------------------------------------
// Repository
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

// ---------------------------------------------------------------------------
// Worktree
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

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
