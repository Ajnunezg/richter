//! `richter agents` — lists detected agent processes with their context.
//!
//! Shows each agent's process ID, current working directory, associated
//! repository or worktree, and the active command they are executing
//! (if forwarded through Richter).

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `agents` subcommand.
#[derive(Args)]
pub struct AgentsArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show only agents with an active run
    #[arg(long)]
    pub active_only: bool,
}

/// An agent process tracked by the Richter daemon.
#[derive(Debug, Serialize, Deserialize)]
struct TrackedAgent {
    /// Process ID of the agent.
    pub pid: u32,
    /// Agent type (e.g., "claude", "codex").
    pub agent_type: String,
    /// Current working directory.
    pub cwd: String,
    /// Repository path the agent is working in, if any.
    pub repo: Option<String>,
    /// Worktree path, if the agent is in an isolated worktree.
    pub worktree: Option<String>,
    /// Active command fingerprint, if the agent is currently running something.
    pub active_command: Option<String>,
    /// When the agent was first detected.
    pub detected_at: String,
    /// Whether the agent is currently waiting on a cached result.
    pub waiting_on_cache: bool,
}

/// Runs the agents query.
pub async fn run(args: AgentsArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let agents = fetch_agents(&client).await?;

    let agents: Vec<TrackedAgent> = agents
        .into_iter()
        .filter(|a| {
            if args.active_only {
                a.active_command.is_some() || a.waiting_on_cache
            } else {
                true
            }
        })
        .collect();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&agents)?);
    } else {
        print_human_agents(&agents);
    }

    Ok(())
}

async fn fetch_agents(client: &LocalClient) -> anyhow::Result<Vec<TrackedAgent>> {
    let req = serde_json::json!({
        "method": "agents",
        "params": {}
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to request agents from daemon")?;
    let agents: Vec<TrackedAgent> =
        serde_json::from_slice(&raw).context("Failed to parse agents response")?;
    Ok(agents)
}

fn print_human_agents(agents: &[TrackedAgent]) {
    if agents.is_empty() {
        println!("No agents detected.");
        return;
    }

    for agent in agents {
        let context = match (&agent.worktree, &agent.repo) {
            (Some(wt), _) => wt.clone(),
            (None, Some(repo)) => repo.clone(),
            (None, None) => agent.cwd.clone(),
        };

        let command = agent.active_command.as_deref().unwrap_or("idle");
        let cache_wait = if agent.waiting_on_cache {
            " (waiting on cache)"
        } else {
            ""
        };
        let _wt_label = agent.worktree.as_ref().map(|_| " [worktree]").unwrap_or("");

        println!(
            "  {} (pid {})  {}:{}{cache_wait}",
            agent.agent_type, agent.pid, context, command
        );
        if let Some(repo) = &agent.repo {
            if agent.worktree.is_some() {
                println!("    repo: {repo}");
            }
        }
        println!("    detected: {}", agent.detected_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_active_only() {
        let agents = vec![
            TrackedAgent {
                pid: 1000,
                agent_type: "claude".into(),
                cwd: "/home/dev/repo".into(),
                repo: Some("/home/dev/repo".into()),
                worktree: None,
                active_command: Some("cargo build".into()),
                detected_at: "2026-01-01T00:00:00Z".into(),
                waiting_on_cache: false,
            },
            TrackedAgent {
                pid: 1001,
                agent_type: "codex".into(),
                cwd: "/home/dev/repo".into(),
                repo: Some("/home/dev/repo".into()),
                worktree: None,
                active_command: None,
                detected_at: "2026-01-01T00:00:00Z".into(),
                waiting_on_cache: false,
            },
        ];
        let active: Vec<_> = agents
            .into_iter()
            .filter(|a| a.active_command.is_some())
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].pid, 1000);
    }
}
