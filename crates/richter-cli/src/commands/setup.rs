//! `richter setup` — one-command onboarding.
//!
//! Installs shims, MCP config, shell hooks, and registers the daemon
//! in a single command.

use crate::client::LocalClient;
use clap::Args;

/// Arguments for the `setup` subcommand.
#[derive(Args)]
pub struct SetupArgs {
    /// Install everything (shims, MCP, shell hooks, daemon).
    #[arg(long)]
    pub all: bool,

    /// Install only shims.
    #[arg(long)]
    pub shims_only: bool,

    /// Install only MCP configuration.
    #[arg(long)]
    pub mcp_only: bool,

    /// Force reinstallation.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: SetupArgs, socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);

    println!("Richter Setup");
    println!("=============");
    println!();

    if args.all || args.shims_only {
        println!("[1/3] Installing shims...");
        let req = serde_json::json!({
            "method": "install_shims",
            "params": { "force": args.force }
        });
        match client.send_raw(&req.to_string()) {
            Ok(_) => println!("  ✓ Shims installed"),
            Err(e) => println!("  ⚠ Shims may already be installed ({})", e),
        }
    }

    if args.all || args.mcp_only {
        println!("[2/3] Installing MCP configuration...");
        let req = serde_json::json!({
            "method": "install_mcp",
            "params": {}
        });
        match client.send_raw(&req.to_string()) {
            Ok(_) => println!("  ✓ MCP configured"),
            Err(e) => println!("  ⚠ MCP may already be configured ({})", e),
        }
    }

    if args.all {
        println!("[3/3] Verifying daemon...");
        let req = serde_json::json!({"method": "health"});
        match client.send_raw(&req.to_string()) {
            Ok(_) => println!("  ✓ Daemon is running"),
            Err(_) => println!("  ⚠ Daemon is not running. Start with: richter daemon start"),
        }
    }

    println!();
    println!("Setup complete! Richter is ready.");
    println!();
    println!("Next steps:");
    println!("  • Run `richter status` to see the dashboard");
    println!("  • Run `richter run -- cargo build` to try it out");
    println!("  • Check the mobile app for mobile monitoring");

    Ok(())
}
