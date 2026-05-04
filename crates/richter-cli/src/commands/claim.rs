//! `richter claim` — file and path lease management.
//!
//! Acquires or releases exclusive file leases via the daemon, preventing
//! concurrent agent modification of the same file(s). Supports TTL-based
//! leases that auto-expire.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;
use serde::{Deserialize, Serialize};

/// Arguments for the `claim` subcommand.
#[derive(Args)]
pub struct ClaimArgs {
    /// Path to the file or directory to claim
    #[arg(required = true)]
    pub path: String,

    /// Lease time-to-live (e.g., "30s", "5m", "1h")
    #[arg(long, default_value = "5m")]
    pub ttl: String,

    /// Agent name claiming the resource
    #[arg(long)]
    pub agent: Option<String>,

    /// Release the claim instead of acquiring
    #[arg(long)]
    pub release: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// List active claims (ignore path argument)
    #[arg(long)]
    pub list: bool,
}

/// Response from a claim operation.
#[derive(Debug, Serialize, Deserialize)]
struct ClaimResponse {
    /// Whether the claim was acquired or released.
    pub success: bool,
    /// The canonical path claimed.
    pub path: String,
    /// The agent holding the claim.
    pub agent: String,
    /// Lease TTL.
    pub ttl: String,
    /// When the lease expires (ISO-8601).
    pub expires_at: String,
    /// Claim token (for release).
    pub claim_id: Option<String>,
    /// If acquisition failed, who holds the claim.
    pub held_by: Option<String>,
}

/// Active claim info for listing.
#[derive(Debug, Serialize, Deserialize)]
struct ActiveClaim {
    pub path: String,
    pub agent: String,
    pub expires_at: String,
    pub claim_id: String,
}

/// Runs the claim operation.
pub async fn run(args: ClaimArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);

    if args.list {
        list_claims(&client, args.json).await?;
        return Ok(());
    }

    // Canonicalize the path
    let path = std::path::PathBuf::from(&args.path);
    let canonical = if path.exists() {
        path.canonicalize()
            .context("Failed to canonicalize path")?
            .to_string_lossy()
            .to_string()
    } else {
        // For paths that don't exist yet, use as-is
        args.path.clone()
    };

    if args.release {
        release_claim(&client, &canonical, args.json).await?;
    } else {
        acquire_claim(
            &client,
            &canonical,
            &args.ttl,
            args.agent.as_deref(),
            args.json,
        )
        .await?;
    }

    Ok(())
}

async fn acquire_claim(
    client: &LocalClient,
    path: &str,
    ttl: &str,
    agent: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let agent_name = agent.unwrap_or("unknown");

    let req = serde_json::json!({
        "method": "claim_acquire",
        "params": {
            "path": path,
            "ttl": ttl,
            "agent": agent_name,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to acquire claim")?;
    let resp: ClaimResponse =
        serde_json::from_slice(&raw).context("Failed to parse claim response")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else if resp.success {
        println!("Claim acquired on: {}", resp.path);
        println!("  Agent:    {}", resp.agent);
        println!("  TTL:      {}", resp.ttl);
        println!("  Expires:  {}", resp.expires_at);
        if let Some(ref id) = resp.claim_id {
            println!("  Claim ID: {id}");
        }
    } else {
        let holder = resp.held_by.as_deref().unwrap_or("unknown");
        println!("Claim FAILED: {} is already held by {holder}", resp.path);
        println!("  Expires: {}", resp.expires_at);
        std::process::exit(1);
    }

    Ok(())
}

async fn release_claim(client: &LocalClient, path: &str, json: bool) -> anyhow::Result<()> {
    let req = serde_json::json!({
        "method": "claim_release",
        "params": {
            "path": path,
        }
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to release claim")?;
    let resp: ClaimResponse =
        serde_json::from_slice(&raw).context("Failed to parse release response")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else if resp.success {
        println!("Claim released: {}", resp.path);
    } else {
        println!("No claim found for: {}", resp.path);
    }

    Ok(())
}

async fn list_claims(client: &LocalClient, json: bool) -> anyhow::Result<()> {
    let req = serde_json::json!({
        "method": "claim_list",
        "params": {}
    });

    let raw = client
        .send_raw(&req.to_string())
        .context("Failed to list claims")?;
    let claims: Vec<ActiveClaim> =
        serde_json::from_slice(&raw).context("Failed to parse claims list")?;

    if json {
        println!("{}", serde_json::to_string_pretty(&claims)?);
    } else if claims.is_empty() {
        println!("No active claims.");
    } else {
        println!("Active claims:");
        for claim in &claims {
            println!(
                "  {}  agent={}  expires={}  id={}",
                claim.path, claim.agent, claim.expires_at, claim.claim_id
            );
        }
    }

    Ok(())
}
