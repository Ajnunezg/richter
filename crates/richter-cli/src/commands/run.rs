//! `richter run` — the core command execution interface.
//!
//! Sends a command to the daemon for execution or cache lookup. Handles
//! joined-existing-run messages, cached results, queued status, and
//! pass-through output streaming. Ctrl-C detaches the subscriber without
//! killing the run (unless the subscriber is the run leader).

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `run` subcommand.
#[derive(Args)]
pub struct RunArgs {
    /// The command to execute (everything after `--`)
    #[arg(trailing_var_arg = true, required = true)]
    pub command: Vec<String>,

    /// Named shim that invoked this command (for shim mode)
    #[arg(long)]
    pub shim_name: Option<String>,

    /// CWD override for the command
    #[arg(long)]
    pub cwd: Option<String>,

    /// Force re-execution even if a cached result exists
    #[arg(long)]
    pub force: bool,

    /// Timeout in seconds
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Wait for completion and output result
    #[arg(long)]
    pub wait: bool,

    /// Submit only; return immediately with run ID
    #[arg(long)]
    pub detach: bool,
}

/// Response from a run submission (matches daemon RunOutcome).
#[derive(Debug, Serialize, Deserialize)]
struct RunResponse {
    /// Outcome type: "Started", "Joined", "Cached", "Queued", "Rejected".
    #[serde(rename = "type")]
    pub outcome_type: String,
    /// Unique run identifier.
    pub run_id: String,
    /// Exit code (only for Cached).
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Cached output text (only for Cached).
    #[serde(default)]
    pub output: Option<String>,
    /// Cache age (only for Cached).
    #[serde(default)]
    pub cache_age: Option<String>,
    /// Estimated wait time (only for Queued).
    #[serde(default)]
    pub estimated_wait_ms: Option<u64>,
    /// Reason for rejection (only for Rejected).
    #[serde(default)]
    pub reason: Option<String>,
    /// Queue time in ms (only for Started).
    #[serde(default)]
    pub queue_time_ms: Option<u64>,
}

/// Runs the core command execution flow.
pub async fn run(args: RunArgs, socket: &str) -> anyhow::Result<()> {
    let command_str = args.command.join(" ");

    if command_str.trim().is_empty() {
        anyhow::bail!("Empty command. Use `richter run -- <command...>`.");
    }

    let client = LocalClient::new(socket);

    let req = serde_json::json!({
        "method": "run",
        "params": {
            "command": command_str,
            "shim_name": args.shim_name,
            "cwd": args.cwd,
            "timeout_secs": args.timeout,
            "wait": args.wait || !args.detach,
            "detach": args.detach,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to send run request to daemon")?;

    let resp: RunResponse =
        serde_json::from_slice(&raw).context("Failed to parse run response from daemon")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
        std::process::exit(exit_code_from_response(&resp));
    }

    match resp.outcome_type.as_str() {
        "Joined" => {
            let reason = resp.reason.as_deref().unwrap_or("unknown");
            println!("Richter: joined existing run {} — {}", resp.run_id, reason);
            println!("Waiting for result...");
        }
        "Cached" => {
            let reason = resp.reason.as_deref().unwrap_or("cache hit");
            let age = resp.cache_age.as_deref().unwrap_or("?");
            println!(
                "Richter: cache hit for run {} — {} (age: {})",
                resp.run_id, reason, age
            );
            if let Some(ref output) = resp.output {
                if !output.is_empty() {
                    print!("{}", output);
                }
            }
            let code = resp.exit_code.unwrap_or(0);
            std::process::exit(code);
        }
        "Queued" => {
            let reason = resp.reason.as_deref().unwrap_or("resource constrained");
            let wait = resp.estimated_wait_ms.unwrap_or(0);
            println!(
                "Richter: run {} queued — {} (est. {}ms)",
                resp.run_id, reason, wait
            );
        }
        "Started" => {
            let reason = resp.reason.as_deref().unwrap_or("new run started");
            let qtime = resp.queue_time_ms.unwrap_or(0);
            println!(
                "Richter: executing run {} — {} (queue time: {}ms)",
                resp.run_id, reason, qtime
            );
        }
        "Rejected" => {
            let reason = resp.reason.as_deref().unwrap_or("unknown reason");
            println!("Richter: run {} REJECTED — {}", resp.run_id, reason);
            std::process::exit(1);
        }
        other => {
            println!("Richter: run {} outcome: {}", resp.run_id, other);
        }
    }

    // If detached, exit now
    if args.detach {
        std::process::exit(0);
    }

    // Wait and stream output
    wait_for_completion(&client, &resp.run_id, args.json).await?;

    Ok(())
}

/// Polls the daemon for run completion and streams output.
async fn wait_for_completion(client: &LocalClient, run_id: &str, json: bool) -> anyhow::Result<()> {
    // Polling loop: ask daemon for run status until complete
    loop {
        let req = serde_json::json!({
            "method": "run_status",
            "params": { "run_id": run_id }
        });

        let raw = client
            .send_raw(&req.to_string())
            .with_context(|| format!("Failed to poll status for run {run_id}"))?;

        let poll_resp: RunStatusResponse =
            serde_json::from_slice(&raw).context("Failed to parse run status")?;

        match poll_resp.status.as_str() {
            "completed" | "failed" | "signalled" => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&poll_resp)?);
                } else {
                    if let Some(ref stdout) = poll_resp.stdout {
                        if !stdout.is_empty() {
                            print!("{stdout}");
                        }
                    }
                    if let Some(ref stderr) = poll_resp.stderr {
                        if !stderr.is_empty() {
                            eprint!("{stderr}");
                        }
                    }
                    let duration = poll_resp.duration_secs.unwrap_or(0.0);
                    let code = poll_resp.exit_code.unwrap_or(-1);
                    println!("\n(completed in {duration:.1}s, exit code {code})");
                }
                std::process::exit(poll_resp.exit_code.unwrap_or(1));
            }
            _ => {
                tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
            }
        }
    }
}

/// Response from the run status polling endpoint.
#[derive(Debug, Serialize, Deserialize)]
struct RunStatusResponse {
    pub status: String,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_secs: Option<f64>,
}

fn exit_code_from_response(resp: &RunResponse) -> i32 {
    resp.exit_code.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_code_from_response() {
        let resp = RunResponse {
            outcome_type: "Cached".into(),
            run_id: "1".into(),
            exit_code: Some(42),
            output: None,
            cache_age: None,
            estimated_wait_ms: None,
            reason: None,
            queue_time_ms: None,
        };
        assert_eq!(exit_code_from_response(&resp), 42);
    }

    #[test]
    fn test_exit_code_from_response_no_cache() {
        let resp = RunResponse {
            outcome_type: "Started".into(),
            run_id: "1".into(),
            exit_code: None,
            output: None,
            cache_age: None,
            estimated_wait_ms: None,
            reason: None,
            queue_time_ms: None,
        };
        assert_eq!(exit_code_from_response(&resp), 0);
    }
}
