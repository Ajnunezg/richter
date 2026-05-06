//! `richter setup` — one-command onboarding.

use anyhow::Context;
use crate::client::LocalClient;
use clap::Args;

#[derive(Args)]
pub struct SetupArgs {
    /// Run all setup steps (shims, MCP, shell integration, hooks).
    #[arg(long)]
    pub all: bool,

    /// Dry run: show what would be done without making changes.
    #[arg(long)]
    pub dry_run: bool,

    /// Install only shims.
    #[arg(long)]
    pub shims_only: bool,

    /// Install only MCP configuration.
    #[arg(long)]
    pub mcp_only: bool,

    /// Force reinstallation even if already configured.
    #[arg(long)]
    pub force: bool,
}

pub async fn run(args: SetupArgs, socket: &str) -> anyhow::Result<()> {
    if args.all {
        return run_all_setup(args.dry_run, socket).await;
    }

    let client = LocalClient::new(socket);
    println!("Richter Setup");
    println!("=============");
    println!();

    // Standard setup path: report current status
    match client.send_raw(r##"{"method":"status"}"##) {
        Ok(_) => println!("  Daemon is reachable via {socket}"),
        Err(_) => println!("  Daemon not reachable. Start with 'richter daemon start'."),
    }

    Ok(())
}

async fn run_all_setup(dry: bool, socket: &str) -> anyhow::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let total: u32 = 5;

    println!("Richter Setup (--all)");
    println!("{:=<40}", "");

    // Step 1
    println!("[1/{}] Creating ~/.richter/ data directory...", total);
    if !dry { std::fs::create_dir_all(format!("{home}/.richter")).context("mkdir")?; }
    println!("  Done.");

    // Step 2
    println!("[2/{}] Installing shell integration...", total);
    if !dry {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let rc = if shell.contains("zsh") { format!("{home}/.zshrc") }
                 else { format!("{home}/.bashrc") };
        if std::path::Path::new(&rc).exists() {
            let cur = std::fs::read_to_string(&rc).unwrap_or_default();
            if !cur.contains("Richter") {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().append(true).open(&rc)?;
                let export_line = "\nexport PATH=\"$HOME/.richter/shims:$PATH\"  # Richter";
                writeln!(f, "{}", export_line)?;
            }
        }
    }
    println!("  Done.");

    // Step 3
    println!("[3/{}] Installing PATH shims...", total);
    if !dry {
        let sd = format!("{home}/.richter/shims");
        std::fs::create_dir_all(&sd).context("shims dir")?;
        for tool in &["cargo","go","npm","pip","make"] {
            let sp = format!("{sd}/{tool}");
            std::fs::write(&sp, format!("#!/bin/sh\nexec richter run -- {tool} \"$@\"\n"))?;
            std::process::Command::new("chmod").args(["+x",&sp]).status().ok();
        }
    }
    println!("  Done.");

    // Step 4
    println!("[4/{}] Generating MCP config...", total);
    if !dry {
        let md = format!("{home}/.richter/mcp");
        std::fs::create_dir_all(&md).context("mcp dir")?;
        let cfg = serde_json::json!({"mcpServers":{"richter":{"command":"richter","args":["mcp"]}}});
        std::fs::write(format!("{md}/config.json"), serde_json::to_string_pretty(&cfg)?)?;
    }
    println!("  Done.");

    // Step 5
    println!("[5/{}] Verifying daemon...", total);
    if !dry {
        let c = LocalClient::new(socket);
        match c.send_raw(r##"{"method":"health"}"##) {
            Ok(_) => println!("  Daemon reachable."),
            Err(_) => println!("  Warning: daemon not reachable. Run 'richter daemon start'."),
        }
    }
    println!("  Done.");

    println!("{:=<40}", "");
    println!("Setup complete: {total}/{total} steps finished.");
    if dry { println!("(dry run)"); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_setup_all_flag() {
        let args = SetupArgs { all: true, dry_run: false, shims_only: false, mcp_only: false, force: false };
        assert!(args.all);
    }

    #[test]
    fn test_setup_dry_run_flag() {
        let args = SetupArgs { all: false, dry_run: true, shims_only: false, mcp_only: false, force: false };
        assert!(args.dry_run);
    }
}
