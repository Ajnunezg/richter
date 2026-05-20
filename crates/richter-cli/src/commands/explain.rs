//! `richter explain` — explain why a run was queued, cached, or joined.
//!
//! Queries the daemon for decision transparency records and presents
//! a human-readable explanation of the disposotion.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `explain` subcommand.
#[derive(Args)]
pub struct ExplainArgs {
    /// The run ID to explain.
    pub run_id: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Explanation response from the daemon.
#[derive(Debug, Serialize, Deserialize)]
struct ExplainResponse {
    pub run_id: String,
    pub command: String,
    pub disposition: String,
    pub reason: String,
    pub fingerprint: String,
    pub cache_age: Option<String>,
    pub queue_position: Option<u64>,
    pub estimated_wait_ms: Option<u64>,
}

pub async fn run(args: ExplainArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);

    let req = serde_json::json!({
        "method": "explain",
        "params": { "run_id": args.run_id }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to send explain request to daemon")?;

    let resp: ExplainResponse =
        serde_json::from_slice(&raw).context("Failed to parse explain response")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        print_human_explain(&resp);
    }

    Ok(())
}

fn print_human_explain(resp: &ExplainResponse) {
    println!("Richter Decision Explanation");
    println!("============================");
    println!();
    println!("  Run ID:      {}", resp.run_id);
    println!("  Command:     {}", resp.command);
    println!("  Disposition: {}", resp.disposition);
    println!("  Fingerprint: {}", resp.fingerprint);
    println!();
    println!("  Why this decision?");
    println!("  {}", resp.reason);
    println!();
    if let Some(ref age) = resp.cache_age {
        println!("  Cache age: {}", age);
    }
    if let Some(pos) = resp.queue_position {
        println!("  Queue position: {}", pos);
    }
    if let Some(wait) = resp.estimated_wait_ms {
        println!("  Estimated wait: {} ms", wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explain_response_serde() {
        let resp = ExplainResponse {
            run_id: "r1".into(),
            command: "cargo build".into(),
            disposition: "cached".into(),
            reason: "fresh cache hit".into(),
            fingerprint: "fp123".into(),
            cache_age: Some("30s".into()),
            queue_position: None,
            estimated_wait_ms: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("cached"));
    }
}
