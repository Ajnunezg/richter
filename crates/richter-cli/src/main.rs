//! Richter CLI: the primary user-facing command-line interface for the Richter system.
//!
//! Richter is a daemon-driven command de-duplication and caching system
//! for agentic coding assistants. This CLI communicates with the
//! `richter-daemon` via a local Unix domain socket to submit commands,
//! query status, diagnose configuration, and manage the system.

use anyhow::Context;

use richter_cli::client;
use richter_cli::commands;

use clap::{Parser, Subcommand};
use tracing_subscriber::{fmt, EnvFilter};

/// Richter CLI — daemon-driven command de-duplication for agentic coding.
#[derive(Parser)]
#[command(
    name = "richter",
    version,
    about = "Daemon-driven command de-duplication and caching for agentic coding assistants",
    long_about = "Richter intercepts shell commands from agentic coding tools (Claude Code, Codex, etc.) \
                  and deduplicates identical concurrent work across agents, repos, and worktrees.",
    arg_required_else_help = true
)]
struct Cli {
    /// Verbose output (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Path to the Richter daemon socket
    #[arg(long, env = "RICHTER_SOCKET", default_value = "", global = true)]
    socket: String,

    /// Subcommand to execute
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Diagnose daemon status, shims, PATH, hooks, MCP, and permissions
    Doctor(commands::doctor::DoctorArgs),

    /// Show global status: active runs, queued runs, cache stats, system pressure
    Status(commands::status::StatusArgs),

    /// List tracked repositories and worktrees
    Repos(commands::repos::ReposArgs),

    /// List detected agents with their current working context
    Agents(commands::agents::AgentsArgs),

    /// Show recent runs with fingerprint, cache status, and outcome
    Runs(commands::runs::RunsArgs),

    /// Show important events or raw event stream
    Events(commands::events::EventsArgs),

    /// Submit a command to the daemon for execution or cache lookup
    #[command(trailing_var_arg = true)]
    Run(commands::run::RunArgs),

    /// Install or uninstall shims, shell integration, MCP config, or agent hooks
    #[command(subcommand)]
    Install(commands::install::InstallCommand),

    /// Simulate N agents running the same command simultaneously
    Simulate(commands::simulate::SimulateArgs),

    /// Claim a file lease for exclusive access
    Claim(commands::claim::ClaimArgs),

    /// Manage agent-specific worktrees
    #[command(subcommand)]
    Worktree(commands::worktree::WorktreeCommand),

    /// Explain why a run was queued, cached, or joined
    Explain(commands::explain::ExplainArgs),

    /// View structured audit log
    Audit(commands::audit::AuditArgs),

    /// One-command onboarding: installs shims, MCP, and verifies daemon
    Setup(commands::setup::SetupArgs),

    /// Manage Richter configuration
    #[command(subcommand)]
    Config(commands::config::ConfigCommand),

    /// Manage the Richter Mobile companion gateway
    #[command(subcommand)]
    Mobile(commands::mobile::MobileCommand),
}

/// Initialize tracing with the configured verbosity level.
fn setup_tracing(verbose: u8) {
    let filter = match verbose {
        0 => EnvFilter::new("richter_cli=info,richter=info"),
        1 => EnvFilter::new("richter_cli=debug,richter=debug"),
        2 => EnvFilter::new("richter_cli=trace,richter=debug"),
        _ => EnvFilter::new("trace"),
    };

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

/// Default set of known build/test/lint tools that can be shimmed.
const DEFAULT_SHIM_TOOLS: &[&str] = &[
    "cargo", "go", "npm", "npx", "yarn", "pnpm", "pip", "pip3", "python", "python3", "make",
    "cmake", "bazel", "rustc", "gcc", "g++", "clang", "clang++", "javac", "dotnet", "git",
];

/// Load the list of shim-able tool names.
/// Merges the built-in defaults with any additional tools specified in
/// `~/.richter/shims.toml` under the `[tools]` key with `additional = [...]`.
fn load_shim_tools() -> Vec<&'static str> {
    let mut tools: Vec<&'static str> = DEFAULT_SHIM_TOOLS.to_vec();
    if let Ok(home) = std::env::var("HOME") {
        let shims_path = std::path::Path::new(&home).join(".richter/shims.toml");
        if shims_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&shims_path) {
                if let Ok(value) = contents.parse::<toml::Value>() {
                    if let Some(additional) = value
                        .get("tools")
                        .and_then(|t| t.get("additional"))
                        .and_then(|a| a.as_array())
                    {
                        for item in additional {
                            if let Some(name) = item.as_str() {
                                if !tools.contains(&name) {
                                    // Leak the string to get a &'static str.
                                    // This is acceptable because the list is loaded once at startup.
                                    let leaked: &'static str =
                                        Box::leak(name.to_string().into_boxed_str());
                                    tools.push(leaked);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    tools
}

/// Detect if the binary was invoked as a shim (symlink with a different name).
/// Uses argv[0] to determine the tool name. Checks against the configurable
/// list of known tools (defaults + `~/.richter/shims.toml` additional tools).
fn detect_shim() -> Option<String> {
    let argv0 = std::env::args().next()?;
    let invoked_as = std::path::Path::new(&argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())?;

    let tools = load_shim_tools();
    if tools.contains(&invoked_as.as_str()) {
        Some(invoked_as)
    } else {
        None
    }
}

/// Run in shim mode: forward the command to the daemon transparently.
fn run_as_shim(shim_name: &str) -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let cmd_str = if args.is_empty() {
        return Ok(());
    } else {
        args.join(" ")
    };

    let socket = std::env::var("RICHTER_SOCKET").unwrap_or_else(|_| default_socket_path());

    let cwd = std::env::current_dir()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| ".".to_string());

    let client = client::LocalClient::new(&socket);

    let req = serde_json::json!({
        "method": "run",
        "params": {
            "command": cmd_str,
            "shim_name": shim_name,
            "cwd": cwd,
            "wait": true,
            "detach": false,
        }
    });

    let raw = match client.send_raw(&req.to_string()) {
        Ok(r) => r,
        Err(_e) => {
            // Daemon unreachable — exec the real command directly
            let status = std::process::Command::new(shim_name)
                .args(&args)
                .status()
                .unwrap_or_else(|_| std::process::ExitStatus::default());
            std::process::exit(status.code().unwrap_or(1));
        }
    };

    // Parse response
    if let Ok(resp) = serde_json::from_slice::<serde_json::Value>(&raw) {
        let outcome_type = resp["type"].as_str().unwrap_or("Started");
        match outcome_type {
            "Cached" => {
                if let Some(output) = resp["output"].as_str() {
                    print!("{}", output);
                }
                let code = resp["exit_code"].as_i64().unwrap_or(0) as i32;
                std::process::exit(code);
            }
            "Started" | "Joined" => {
                // Run completed — we already waited. Output is in run result.
                // For now, just exit with success since daemon handled it.
                std::process::exit(0);
            }
            "Queued" => {
                eprintln!("[richter] Command queued — system under load");
                std::process::exit(0);
            }
            _ => {
                std::process::exit(0);
            }
        }
    }

    Ok(())
}
fn default_socket_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{home}/.richter/daemon.sock")
}

fn resolve_socket(cli_socket: &str) -> String {
    if cli_socket.is_empty() {
        default_socket_path()
    } else {
        cli_socket.to_string()
    }
}

fn main() -> anyhow::Result<()> {
    // Shim detection: if invoked via a symlink (e.g., `cargo` → richter),
    // forward the command through the daemon automatically.
    if let Some(shim_name) = detect_shim() {
        return run_as_shim(&shim_name);
    }

    let cli = Cli::parse();
    let socket = resolve_socket(&cli.socket);
    setup_tracing(cli.verbose);

    let rt = tokio::runtime::Runtime::new().context("Failed to create Tokio runtime")?;

    rt.block_on(async move {
        match cli.command {
            Commands::Doctor(args) => commands::doctor::run(args, &socket).await,
            Commands::Status(args) => commands::status::run(args, &socket).await,
            Commands::Repos(args) => commands::repos::run(args, &socket).await,
            Commands::Agents(args) => commands::agents::run(args, &socket).await,
            Commands::Runs(args) => commands::runs::run(args, &socket).await,
            Commands::Events(args) => commands::events::run(args, &socket).await,
            Commands::Run(args) => commands::run::run(args, &socket).await,
            Commands::Install(cmd) => commands::install::run(cmd, &socket).await,
            Commands::Simulate(args) => commands::simulate::run(args, &socket).await,
            Commands::Claim(args) => commands::claim::run(args, &socket).await,
            Commands::Worktree(cmd) => commands::worktree::run(cmd, &socket).await,
            Commands::Explain(args) => commands::explain::run(args, &socket).await,
            Commands::Audit(args) => commands::audit::run(args, &socket).await,
            Commands::Setup(args) => commands::setup::run(args, &socket).await,
            Commands::Config(cmd) => commands::config::run(cmd, &socket).await,
            Commands::Mobile(cmd) => commands::mobile::run(cmd, &socket).await,
        }
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn verify_cli_decl() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_cli_help_contains_subcommands() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("doctor"));
        assert!(help.contains("status"));
        assert!(help.contains("repos"));
        assert!(help.contains("agents"));
        assert!(help.contains("runs"));
        assert!(help.contains("events"));
        assert!(help.contains("run"));
        assert!(help.contains("install"));
        assert!(help.contains("simulate"));
        assert!(help.contains("claim"));
        assert!(help.contains("worktree"));
    }
}
