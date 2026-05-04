//! `richter repos` — lists tracked repositories and managed worktrees.
//!
//! Shows each repo's branch, dirty state, and any active agents
//! currently operating within that repo or worktree.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `repos` subcommand.
#[derive(Args)]
pub struct ReposArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show only repos with active agents
    #[arg(long)]
    pub active_only: bool,

    /// Filter by repo path substring
    #[arg(long)]
    pub filter: Option<String>,
}

/// A tracked repository as reported by the daemon.
#[derive(Debug, Serialize, Deserialize)]
struct TrackedRepo {
    /// Absolute path to the repository root.
    pub path: String,
    /// Current branch name.
    pub branch: String,
    /// Whether the working tree has uncommitted changes.
    pub dirty: bool,
    /// Managed worktrees under this repo.
    pub worktrees: Vec<WorktreeInfo>,
    /// Agent names active in this repo or its worktrees.
    pub active_agents: Vec<String>,
}

/// A managed worktree within a repository.
#[derive(Debug, Serialize, Deserialize)]
struct WorktreeInfo {
    /// Path to the worktree directory.
    pub path: String,
    /// Name of the branch this worktree works from.
    pub branch: String,
    /// Agent name associated with this worktree.
    pub agent: Option<String>,
    /// Whether the worktree has uncommitted changes.
    pub dirty: bool,
}

/// Runs the repos query.
pub async fn run(args: ReposArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let repos = fetch_repos(&client).await?;

    let repos: Vec<TrackedRepo> = repos
        .into_iter()
        .filter(|r| {
            if args.active_only && r.active_agents.is_empty() {
                return false;
            }
            if let Some(ref filter) = args.filter {
                return r.path.contains(filter.as_str());
            }
            true
        })
        .collect();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&repos)?);
    } else {
        print_human_repos(&repos);
    }

    Ok(())
}

async fn fetch_repos(client: &LocalClient) -> anyhow::Result<Vec<TrackedRepo>> {
    let req = serde_json::json!({
        "method": "repos",
        "params": {}
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to request repos from daemon")?;
    let repos: Vec<TrackedRepo> =
        serde_json::from_slice(&raw).context("Failed to parse repos response")?;
    Ok(repos)
}

fn print_human_repos(repos: &[TrackedRepo]) {
    if repos.is_empty() {
        println!("No tracked repositories.");
        return;
    }

    for repo in repos {
        let dirty_mark = if repo.dirty { "*" } else { " " };
        let agents = if repo.active_agents.is_empty() {
            String::new()
        } else {
            format!("  agents: {}", repo.active_agents.join(", "))
        };

        println!("{dirty_mark} {} ({}){}", repo.path, repo.branch, agents);

        for wt in &repo.worktrees {
            let wt_dirty = if wt.dirty { "*" } else { " " };
            let wt_agent = wt.agent.as_deref().unwrap_or("-");
            println!(
                "  {wt_dirty} [worktree] {} ({})  agent: {wt_agent}",
                wt.path, wt.branch
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_active_only_empty() {
        let repos = vec![TrackedRepo {
            path: "/home/dev/repo".into(),
            branch: "main".into(),
            dirty: false,
            worktrees: vec![],
            active_agents: vec![],
        }];
        // Simulate filter: active_only should exclude this
        let filtered: Vec<_> = repos
            .into_iter()
            .filter(|r| !r.active_agents.is_empty())
            .collect();
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_by_path() {
        let repos = vec![
            TrackedRepo {
                path: "/home/dev/foo".into(),
                branch: "main".into(),
                dirty: false,
                worktrees: vec![],
                active_agents: vec![],
            },
            TrackedRepo {
                path: "/home/dev/bar".into(),
                branch: "main".into(),
                dirty: false,
                worktrees: vec![],
                active_agents: vec![],
            },
        ];
        let filtered: Vec<_> = repos
            .into_iter()
            .filter(|r| r.path.contains("foo"))
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "/home/dev/foo");
    }
}
