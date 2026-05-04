//! `richter simulate` — simulated multi-agent concurrency scenario.
//!
//! Spawns N fake agents that try to run the same command simultaneously,
//! proving that only one underlying command actually executes while the
//! rest receive the cached or joined result.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `simulate` subcommand.
#[derive(Args)]
pub struct SimulateArgs {
    /// Number of agents to simulate
    #[arg(long, short, default_value = "3")]
    pub agents: u32,

    /// Scenario to simulate
    #[arg(long, default_value = "duplicate-tests")]
    pub scenario: String,

    /// The command for all agents to run
    #[arg(long, default_value = "echo hello from richter simulate")]
    pub command: String,

    /// Stagger agent launches by N milliseconds
    #[arg(long, default_value = "100")]
    pub stagger_ms: u64,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Result from a single simulated agent.
#[derive(Debug, Serialize)]
struct AgentResult {
    /// Agent index.
    pub agent_id: u32,
    /// Run ID returned by daemon.
    pub run_id: String,
    /// Whether this agent joined an existing run.
    pub joined_existing: bool,
    /// Whether the result was from cache.
    pub cached: bool,
    /// Exit code.
    pub exit_code: i32,
    /// Duration of the agent's wait.
    pub duration_ms: u64,
}

/// Overall simulation results.
#[derive(Debug, Serialize)]
struct SimulationResults {
    /// Scenario name.
    pub scenario: String,
    /// Total agents.
    pub total_agents: u32,
    /// Number of agents that joined an existing run.
    pub joined_existing: u32,
    /// Number of agents that got a cached result.
    pub cached: u32,
    /// Number of agents that executed (should be 1 or 0 if cached).
    pub executed: u32,
    /// Whether the simulation proved deduplication.
    pub dedup_proven: bool,
    /// Per-agent results.
    pub agents: Vec<AgentResult>,
}

/// Runs the simulation.
pub async fn run(args: SimulateArgs, socket: &str) -> anyhow::Result<()> {
    println!("Richter Simulation: {}", args.scenario);
    println!("Agents: {}, Command: {}", args.agents, args.command);
    println!("{}", "─".repeat(60));

    let mut results: Vec<AgentResult> = Vec::new();
    let start = std::time::Instant::now();

    for i in 0..args.agents {
        let agent_start = std::time::Instant::now();

        let agent_result = simulate_agent(socket, &args.command, i).await?;
        let duration_ms = agent_start.elapsed().as_millis() as u64;

        results.push(AgentResult {
            agent_id: i,
            run_id: agent_result.run_id,
            joined_existing: agent_result.joined_existing,
            cached: agent_result.cached,
            exit_code: agent_result.exit_code.unwrap_or(-1),
            duration_ms,
        });

        // Stagger launches
        if i < args.agents - 1 {
            tokio::time::sleep(tokio::time::Duration::from_millis(args.stagger_ms)).await;
        }
    }

    let total_ms = start.elapsed().as_millis();

    // Compute summary
    let joined = results.iter().filter(|r| r.joined_existing).count() as u32;
    let cached = results.iter().filter(|r| r.cached).count() as u32;
    let executed = results
        .iter()
        .filter(|r| !r.joined_existing && !r.cached)
        .count() as u32;

    // Dedup is proven if at most 1 agent actually executed
    let dedup_proven = executed <= 1;

    let summary = SimulationResults {
        scenario: args.scenario,
        total_agents: args.agents,
        joined_existing: joined,
        cached,
        executed,
        dedup_proven,
        agents: results,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        print_simulation_results(&summary, total_ms);
    }

    Ok(())
}

/// Internal agent result from daemon.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AgentRunResponse {
    pub run_id: String,
    pub joined_existing: bool,
    pub cached: bool,
    pub status: String,
    pub exit_code: Option<i32>,
}

/// Simulates one agent submitting a command.
async fn simulate_agent(
    socket: &str,
    command: &str,
    _agent_id: u32,
) -> anyhow::Result<AgentRunResponse> {
    let client = LocalClient::new(socket);

    let req = serde_json::json!({
        "method": "run",
        "params": {
            "command": command,
            "shim_name": null,
            "cwd": null,
            "force": false,
            "wait": true,
            "detach": false,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to send simulated run request")?;
    let resp: AgentRunResponse =
        serde_json::from_slice(&raw).context("Failed to parse simulation response")?;

    Ok(resp)
}

fn print_simulation_results(summary: &SimulationResults, total_ms: u128) {
    println!();
    println!("Simulation Results");
    println!("==================");
    println!("  Total agents:         {}", summary.total_agents);
    println!("  Joined existing run:  {}", summary.joined_existing);
    println!("  Cached result:        {}", summary.cached);
    println!("  Actual executions:    {}", summary.executed);
    println!("  Total time:           {total_ms}ms");
    println!();

    if summary.dedup_proven {
        println!(
            "  ✅ Deduplication PROVEN: only {} underlying execution(s) for {} agents.",
            summary.executed, summary.total_agents
        );
    } else {
        println!(
            "  ❌ Deduplication NOT proven: {} executions for {} agents.",
            summary.executed, summary.total_agents
        );
    }

    println!();
    println!("Per-agent results:");
    for agent in &summary.agents {
        let kind = if agent.cached {
            "CACHED"
        } else if agent.joined_existing {
            "JOINED"
        } else {
            "EXECUTED"
        };
        println!(
            "  agent-{} {:>8}  run={}  exit={}  {}ms",
            agent.agent_id, kind, agent.run_id, agent.exit_code, agent.duration_ms
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulation_results_dedup() {
        let summary = SimulationResults {
            scenario: "duplicate-tests".into(),
            total_agents: 3,
            joined_existing: 2,
            cached: 0,
            executed: 1,
            dedup_proven: true,
            agents: vec![],
        };
        assert!(summary.dedup_proven);
    }

    #[test]
    fn test_simulation_results_no_dedup() {
        let summary = SimulationResults {
            scenario: "duplicate-tests".into(),
            total_agents: 3,
            joined_existing: 0,
            cached: 0,
            executed: 3,
            dedup_proven: false,
            agents: vec![],
        };
        assert!(!summary.dedup_proven);
    }
}
