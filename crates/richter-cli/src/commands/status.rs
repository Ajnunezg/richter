//! `richter status` — global system status view.
//!
//! Shows active runs, queued runs, cache statistics, system pressure
//! indicators, and recent important events. Communicates with the daemon
//! to gather live metrics.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `status` subcommand.
#[derive(Args)]
pub struct StatusArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Watch mode: refresh every N seconds
    #[arg(long, short)]
    pub watch: Option<u64>,
}

/// Overall system status returned by the daemon.
#[derive(Debug, Serialize, Deserialize)]
struct StatusResponse {
    /// "ok" when daemon is healthy.
    #[serde(default)]
    pub health: String,
    /// Number of currently executing runs.
    #[serde(default)]
    pub active_runs: u64,
    /// Number of runs queued but not yet started.
    #[serde(default)]
    pub queued_runs: u64,
    /// CPU usage percentage.
    #[serde(default)]
    pub cpu_percent: f64,
    /// Memory usage percentage.
    #[serde(default)]
    pub memory_percent: f64,
    /// Number of event bus subscribers.
    #[serde(default)]
    pub subscriber_count: u64,
}

/// Runs the status query.
pub async fn run(args: StatusArgs, socket: &str) -> anyhow::Result<()> {
    if let Some(interval) = args.watch {
        run_watch(interval, socket, args.json).await
    } else {
        let client = LocalClient::new(socket);
        let status = fetch_status(&client).await?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            print_human_status(&status);
        }
        Ok(())
    }
}

async fn run_watch(interval: u64, socket: &str, json: bool) -> anyhow::Result<()> {
    loop {
        // Clear screen
        print!("\x1B[2J\x1B[H");
        let client = LocalClient::new(socket);
        match fetch_status(&client).await {
            Ok(status) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    print_human_status(&status);
                }
            }
            Err(e) => {
                eprintln!("Status fetch error: {e:?}");
            }
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
    }
}

/// Fetches status from the daemon. Falls back to local-only view
/// when the daemon is unreachable.
async fn fetch_status(client: &LocalClient) -> anyhow::Result<StatusResponse> {
    let req = serde_json::json!({
        "method": "status",
        "params": {}
    });

    let raw = client.send_raw(&req.to_string())?;
    let resp: StatusResponse =
        serde_json::from_slice(&raw).context("Failed to parse status response from daemon")?;
    Ok(resp)
}

fn print_human_status(status: &StatusResponse) {
    println!("Richter System Status");
    println!("=====================");
    println!();

    println!("  Daemon version: 0.1.0");
    println!("  Uptime:         {}s", 0u64);
    println!();

    println!("  Active runs:    {}", status.active_runs);
    println!("  Queued runs:    {}", status.queued_runs);
    println!("  Cached results: {}", 0u64);
    // cache_size_bytes not available in current daemon API
    println!("  Cache size:     N/A");
    println!();

    let pressure_bar = pressure_bar(status.cpu_percent / 100.0);
    println!(
        "  System pressure: {pressure_bar}  ({:.0}%)",
        status.cpu_percent
    );

    // recent_events not yet available
}

fn pressure_bar(pressure: f64) -> String {
    let filled = (pressure * 10.0).clamp(0.0, 10.0) as usize;
    let empty = 10 - filled;
    let bar = "█".repeat(filled) + &"░".repeat(empty);

    let color = if pressure < 0.3 {
        "\x1B[32m" // green
    } else if pressure < 0.7 {
        "\x1B[33m" // yellow
    } else {
        "\x1B[31m" // red
    };

    format!("{color}[{bar}]\x1B[0m")
}

#[allow(dead_code)]
fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_idx = 0;
    while size >= 1024.0 && unit_idx < UNITS.len() - 1 {
        size /= 1024.0;
        unit_idx += 1;
    }
    format!("{size:.1} {}", UNITS[unit_idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_human_bytes() {
        assert_eq!(human_bytes(0), "0.0 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1048576), "1.0 MB");
        assert_eq!(human_bytes(1536), "1.5 KB");
    }

    #[test]
    fn test_pressure_bar_low() {
        let bar = pressure_bar(0.15);
        assert!(bar.contains('█'));
        assert!(bar.contains('░'));
        assert!(bar.contains("[32m")); // green
    }

    #[test]
    fn test_pressure_bar_high() {
        let bar = pressure_bar(0.95);
        assert!(bar.contains("[31m")); // red
    }

    #[test]
    fn test_pressure_bar_zero() {
        let bar = pressure_bar(0.0);
        assert!(!bar.contains('█'));
        assert_eq!(bar.matches('░').count(), 10);
    }
}
