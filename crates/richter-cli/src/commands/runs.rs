//! `richter runs` — shows recent runs with metadata.
//!
//! Displays command, fingerprint, cache status (hit/miss), subscriber
//! count, outcome (success/failure/signal), and timing information.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `runs` subcommand.
#[derive(Args)]
pub struct RunsArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Maximum number of runs to show
    #[arg(long, short, default_value = "20")]
    pub limit: usize,

    /// Show only runs that hit the cache
    #[arg(long)]
    pub cached_only: bool,

    /// Show only runs that were executed (not cached)
    #[arg(long)]
    pub executed_only: bool,

    /// Show only runs matching this fingerprint prefix
    #[arg(long)]
    pub fingerprint: Option<String>,
}

/// A completed or in-progress run.
#[derive(Debug, Serialize, Deserialize)]
struct RunSummary {
    /// Unique run identifier.
    pub run_id: String,
    /// The command that was executed.
    pub command: String,
    /// Fingerprint of the command.
    pub fingerprint: String,
    /// Whether the result came from cache.
    pub cached: bool,
    /// Number of subscribers that watched this run.
    pub subscribers: u64,
    /// Outcome: "success", "failure", "signal", or "running".
    pub outcome: String,
    /// Exit code, if applicable.
    pub exit_code: Option<i32>,
    /// Duration in seconds, if completed.
    pub duration_secs: Option<f64>,
    /// When the run was created.
    pub created_at: String,
    /// Repository associated with the run.
    pub repo: Option<String>,
}

/// Runs the runs query.
pub async fn run(args: RunsArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let runs = fetch_runs(&client, args.limit).await?;

    let runs: Vec<RunSummary> = runs
        .into_iter()
        .filter(|r| {
            if args.cached_only && !r.cached {
                return false;
            }
            if args.executed_only && r.cached {
                return false;
            }
            if let Some(ref fp_prefix) = args.fingerprint {
                return r.fingerprint.starts_with(fp_prefix.as_str());
            }
            true
        })
        .collect();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&runs)?);
    } else {
        print_human_runs(&runs);
    }

    Ok(())
}

async fn fetch_runs(client: &LocalClient, limit: usize) -> anyhow::Result<Vec<RunSummary>> {
    let req = serde_json::json!({
        "method": "runs",
        "params": { "limit": limit }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to request runs from daemon")?;
    let runs: Vec<RunSummary> =
        serde_json::from_slice(&raw).context("Failed to parse runs response")?;
    Ok(runs)
}

fn print_human_runs(runs: &[RunSummary]) {
    if runs.is_empty() {
        println!("No runs recorded.");
        return;
    }

    // Header
    println!(
        "{:<10} {:<10} {:<8} {:<20} {:<12} COMMAND",
        "STATUS", "CACHE", "CODE", "FINGERPRINT", "SUBS"
    );
    println!("{}", "─".repeat(80));

    for run in runs {
        let status = match run.outcome.as_str() {
            "success" => "OK",
            "failure" => "FAIL",
            "signal" => "SIG",
            "running" => "RUNNING",
            _ => "?",
        };
        let cache = if run.cached { "HIT" } else { "MISS" };
        let code = run
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        let fp_short = &run.fingerprint[..run.fingerprint.len().min(16)];

        println!(
            "{status:<10} {cache:<10} {code:<8} {fp_short:<20} {subs:<12} {cmd}",
            status = status,
            cache = cache,
            code = code,
            fp_short = fp_short,
            subs = run.subscribers,
            cmd = truncate_str(&run.command, 40),
        );
    }
}

fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        &s[..max_len - 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 8), "hello");
    }

    #[test]
    fn test_filter_cached_only() {
        let runs = vec![
            RunSummary {
                run_id: "1".into(),
                command: "echo hi".into(),
                fingerprint: "abc123".into(),
                cached: true,
                subscribers: 1,
                outcome: "success".into(),
                exit_code: Some(0),
                duration_secs: Some(0.1),
                created_at: "2026-01-01T00:00:00Z".into(),
                repo: None,
            },
            RunSummary {
                run_id: "2".into(),
                command: "echo bye".into(),
                fingerprint: "def456".into(),
                cached: false,
                subscribers: 1,
                outcome: "success".into(),
                exit_code: Some(0),
                duration_secs: Some(0.1),
                created_at: "2026-01-01T00:00:00Z".into(),
                repo: None,
            },
        ];
        let cached: Vec<_> = runs.into_iter().filter(|r| r.cached).collect();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].run_id, "1");
    }

    #[test]
    fn test_filter_fingerprint_prefix() {
        let runs = vec![
            RunSummary {
                run_id: "1".into(),
                command: "cmd1".into(),
                fingerprint: "abcd1234".into(),
                cached: false,
                subscribers: 0,
                outcome: "success".into(),
                exit_code: Some(0),
                duration_secs: None,
                created_at: "".into(),
                repo: None,
            },
            RunSummary {
                run_id: "2".into(),
                command: "cmd2".into(),
                fingerprint: "efgh5678".into(),
                cached: false,
                subscribers: 0,
                outcome: "success".into(),
                exit_code: Some(0),
                duration_secs: None,
                created_at: "".into(),
                repo: None,
            },
        ];
        let filtered: Vec<_> = runs
            .into_iter()
            .filter(|r| r.fingerprint.starts_with("abcd"))
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].run_id, "1");
    }
}
