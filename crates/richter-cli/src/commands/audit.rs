//! `richter audit` — structured audit log viewer.
//!
//! Shows recent daemon decisions, run outcomes, and security events
//! in a timestamped, filterable format.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `audit` subcommand.
#[derive(Args)]
pub struct AuditArgs {
    /// Show the last N entries (default 50).
    #[arg(long, default_value = "50")]
    pub last: usize,

    /// Filter by event type (e.g. "decision", "run", "security").
    #[arg(long)]
    pub kind: Option<String>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuditEntry {
    pub id: String,
    pub event_type: String,
    pub title: String,
    pub summary: String,
    pub severity: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuditResponse {
    pub entries: Vec<AuditEntry>,
    pub total: usize,
}

pub async fn run(args: AuditArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);

    let req = serde_json::json!({
        "method": "audit",
        "params": {
            "last": args.last,
            "kind": args.kind,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to send audit request to daemon")?;

    let resp: AuditResponse =
        serde_json::from_slice(&raw).context("Failed to parse audit response")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        println!("Richter Audit Log (last {})", resp.total.min(args.last));
        println!("{}", "=".repeat(60));
        for entry in &resp.entries {
            let kind_icon = match entry.event_type.as_str() {
                "decision" => "[DECIDE]",
                "security" => "[SECURE]",
                "run_started" => "[START] ",
                "run_completed" => "[DONE]  ",
                "run_cached" => "[CACHE] ",
                _ => "[EVENT] ",
            };
            println!(
                "{} {} | {} | {}",
                entry.created_at, kind_icon, entry.severity, entry.title
            );
            if !entry.summary.is_empty() {
                println!("   {}", entry.summary);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_response_serde() {
        let resp = AuditResponse {
            entries: vec![AuditEntry {
                id: "e1".into(),
                event_type: "decision".into(),
                title: "Cache hit".into(),
                summary: "served from database".into(),
                severity: "info".into(),
                created_at: "2024-01-01T00:00:00Z".into(),
            }],
            total: 1,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("Cache hit"));
    }
}
