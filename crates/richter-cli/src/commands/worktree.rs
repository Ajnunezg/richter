//! `richter worktree` — managed worktree operations.
//!
//! Creates, lists, and removes agent-specific worktrees via the daemon.
//! Worktrees are isolated checkouts suitable for concurrent agent sessions
//! within the same repository.

use crate::client::LocalClient;
use anyhow::Context;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

/// Subcommand group for worktree management.
#[derive(Subcommand)]
pub enum WorktreeCommand {
    /// Create a new managed worktree for an agent
    Create(WorktreeCreateArgs),
    /// List managed worktrees
    List(WorktreeListArgs),
    /// Remove a managed worktree
    Remove(WorktreeRemoveArgs),
}

/// Arguments for `worktree create`.
#[derive(Args)]
pub struct WorktreeCreateArgs {
    /// Agent name the worktree is for
    #[arg(long, required = true)]
    pub agent: String,

    /// Source branch to create the worktree from
    #[arg(long, required = true)]
    pub from: String,

    /// Repository path (auto-detected from CWD if not specified)
    #[arg(long)]
    pub repo: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `worktree list`.
#[derive(Args)]
pub struct WorktreeListArgs {
    /// Filter by agent name
    #[arg(long)]
    pub agent: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `worktree remove`.
#[derive(Args)]
pub struct WorktreeRemoveArgs {
    /// Path to the worktree to remove
    #[arg(required = true)]
    pub path: String,

    /// Force removal even if the worktree has uncommitted changes
    #[arg(long)]
    pub force: bool,
}

/// A managed worktree as reported by the daemon.
#[derive(Debug, Serialize, Deserialize)]
struct WorktreeEntry {
    /// Path to the worktree directory.
    pub path: String,
    /// Repository this worktree belongs to.
    pub repo: String,
    /// Branch the worktree was created from.
    pub from_branch: String,
    /// Agent associated with this worktree.
    pub agent: String,
    /// Whether the worktree has uncommitted changes.
    pub dirty: bool,
    /// When the worktree was created.
    pub created_at: String,
}

/// Runs the worktree command.
pub async fn run(cmd: WorktreeCommand, socket: &str) -> anyhow::Result<()> {
    match cmd {
        WorktreeCommand::Create(args) => create_worktree(args, socket).await,
        WorktreeCommand::List(args) => list_worktrees(args, socket).await,
        WorktreeCommand::Remove(args) => remove_worktree(args, socket).await,
    }
}

async fn create_worktree(args: WorktreeCreateArgs, socket: &str) -> anyhow::Result<()> {
    let repo = args.repo.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    });

    let client = LocalClient::new(socket);
    let req = serde_json::json!({
        "method": "worktree_create",
        "params": {
            "agent": args.agent,
            "repo": repo,
            "from_branch": args.from,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to create worktree")?;

    let entry: WorktreeEntry =
        serde_json::from_slice(&raw).context("Failed to parse worktree create response")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entry)?);
    } else {
        println!("Worktree created:");
        println!("  Path:       {}", entry.path);
        println!("  Repo:       {}", entry.repo);
        println!("  From branch: {}", entry.from_branch);
        println!("  Agent:      {}", entry.agent);
    }

    Ok(())
}

async fn list_worktrees(args: WorktreeListArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let req = serde_json::json!({
        "method": "worktree_list",
        "params": {}
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to list worktrees")?;

    let mut entries: Vec<WorktreeEntry> =
        serde_json::from_slice(&raw).context("Failed to parse worktree list")?;

    if let Some(ref agent_filter) = args.agent {
        entries.retain(|e| e.agent == *agent_filter);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else if entries.is_empty() {
        println!("No managed worktrees.");
    } else {
        println!("Managed worktrees:");
        for wt in &entries {
            let dirty_mark = if wt.dirty { " *" } else { "" };
            println!(
                "  {dirty_mark} {}  agent={}  from={}",
                wt.path, wt.agent, wt.from_branch
            );
        }
    }

    Ok(())
}

async fn remove_worktree(args: WorktreeRemoveArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let req = serde_json::json!({
        "method": "worktree_remove",
        "params": {
            "path": args.path,
            "force": args.force,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to remove worktree")?;

    // Response is a simple status object
    let resp: serde_json::Value =
        serde_json::from_slice(&raw).context("Failed to parse worktree remove response")?;

    let success = resp
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if success {
        println!("Worktree removed: {}", args.path);
    } else {
        let msg = resp
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("Failed to remove worktree: {msg}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_entry_fields() {
        let entry = WorktreeEntry {
            path: "/tmp/wt".into(),
            repo: "/home/dev/repo".into(),
            from_branch: "main".into(),
            agent: "claude".into(),
            dirty: false,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        assert_eq!(entry.agent, "claude");
        assert_eq!(entry.from_branch, "main");
    }

    #[test]
    fn test_filter_by_agent() {
        let entries = vec![
            WorktreeEntry {
                path: "/tmp/wt1".into(),
                repo: "/repo".into(),
                from_branch: "main".into(),
                agent: "claude".into(),
                dirty: false,
                created_at: "".into(),
            },
            WorktreeEntry {
                path: "/tmp/wt2".into(),
                repo: "/repo".into(),
                from_branch: "feat/x".into(),
                agent: "codex".into(),
                dirty: false,
                created_at: "".into(),
            },
        ];
        let filtered: Vec<_> = entries.into_iter().filter(|e| e.agent == "codex").collect();
        assert_eq!(filtered.len(), 1);
    }
}
