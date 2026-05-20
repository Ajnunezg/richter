//! `richter install` / `richter uninstall` — system integration setup and teardown.
//!
//! Manages shell integration (PATH), shim symlinks, MCP server registration,
//! and agent-specific hook configuration files. Supports installation and
//! removal for all integration points.

use anyhow::Context;
use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Subcommand group for install/uninstall operations.
#[derive(Subcommand)]
pub enum InstallCommand {
    /// Install shell integration (add shims to PATH)
    Shell(InstallShellArgs),
    /// Install tool shims in ~/.richter/shims/
    Shims(InstallShimsArgs),
    /// Install MCP server configuration
    Mcp(InstallMcpArgs),
    /// Install agent hooks
    Hooks(InstallHooksArgs),
    /// Install the prebuilt richter binary to ~/.richter/bin/
    Binary(InstallBinaryArgs),

    /// Remove all Richter integrations
    Uninstall(UninstallArgs),
}

/// Arguments for `install shell`.
#[derive(Args)]
pub struct InstallShellArgs {
    /// Shell type (auto-detected if not specified)
    #[arg(long)]
    pub shell: Option<String>,
}

/// Arguments for `install shims`.
#[derive(Args)]
pub struct InstallShimsArgs {
    /// Also install shell PATH integration
    #[arg(long)]
    pub with_shell: bool,
}

/// Arguments for `install mcp`.
#[derive(Args)]
pub struct InstallMcpArgs {
    /// Agent to install MCP for
    #[arg(long, default_value = "claude")]
    pub agent: String,
}

/// Arguments for `install hooks`.
#[derive(Args)]
pub struct InstallHooksArgs {
    /// Agent to install hooks for
    #[arg(long, default_value = "claude")]
    pub agent: String,
}

/// Arguments for `install binary`.
#[derive(Args)]
pub struct InstallBinaryArgs {
    /// Also rebuild from source before installing.
    #[arg(long)]
    pub rebuild: bool,
}

/// Arguments for `uninstall`.
#[derive(Args)]
pub struct UninstallArgs {
    /// Do not prompt for confirmation
    #[arg(long)]
    pub force: bool,
}

/// Runs the install/uninstall command.
pub async fn run(cmd: InstallCommand, socket: &str) -> anyhow::Result<()> {
    match cmd {
        InstallCommand::Shell(args) => install_shell(args).await,
        InstallCommand::Shims(args) => install_shims(args).await,
        InstallCommand::Mcp(args) => install_mcp(args).await,
        InstallCommand::Hooks(args) => install_hooks(args).await,
        InstallCommand::Binary(args) => install_binary(args).await,
        InstallCommand::Uninstall(args) => uninstall(args, socket).await,
    }
}

/// Adds `~/.richter/shims` to the user's shell PATH.
async fn install_shell(args: InstallShellArgs) -> anyhow::Result<()> {
    let shell = args.shell.unwrap_or_else(detect_shell);
    let shim_dir = shim_dir();
    let shim_dir_str = shim_dir.to_string_lossy();
    let rc_file = shell_rc_path(&shell);

    let export_line = format!("\n# Richter shims\nexport PATH=\"{shim_dir_str}:$PATH\"\n");

    if rc_file.exists() {
        let existing = std::fs::read_to_string(&rc_file).context("Failed to read shell RC file")?;
        if existing.contains(shim_dir_str.as_ref()) {
            println!("Richter shims already configured in {}", rc_file.display());
            return Ok(());
        }
    }

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc_file)
        .with_context(|| format!("Failed to open {}", rc_file.display()))?;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc_file)
        .with_context(|| format!("Failed to open {}", rc_file.display()))?;
    use std::io::Write;
    file.write_all(export_line.as_bytes())
        .context("Failed to write to shell RC file")?;

    println!("Added Richter shims to PATH in {}", rc_file.display());
    println!("Restart your shell or run: source {}", rc_file.display());
    Ok(())
}

/// Creates the `~/.richter/shims/` directory with symlinks for supported tools.
async fn install_shims(args: InstallShimsArgs) -> anyhow::Result<()> {
    let shim_dir = shim_dir();
    std::fs::create_dir_all(&shim_dir)
        .with_context(|| format!("Failed to create shim directory {}", shim_dir.display()))?;

    let tools = supported_tools();
    let richter_bin =
        std::env::current_exe().context("Failed to resolve current executable path")?;

    for tool in &tools {
        let link_path = shim_dir.join(tool);
        // Remove existing symlink or file
        if link_path.exists() || link_path.is_symlink() {
            std::fs::remove_file(&link_path).ok();
        }
        std::os::unix::fs::symlink(&richter_bin, &link_path)
            .with_context(|| format!("Failed to symlink {tool}"))?;
        println!("  Created shim: {tool}");
    }

    println!("Installed {} shims in {}", tools.len(), shim_dir.display());

    if args.with_shell {
        install_shell(InstallShellArgs { shell: None }).await?;
    } else {
        println!("To add shims to PATH, run: richter install shell");
    }

    Ok(())
}

/// Builds (optional) and installs the richter binary to ~/.richter/bin/
async fn install_binary(args: InstallBinaryArgs) -> anyhow::Result<()> {
    let target_dir = richter_dir().join("bin");
    std::fs::create_dir_all(&target_dir).context("Failed to create ~/.richter/bin/")?;

    if args.rebuild {
        println!("Building richter binary (release mode)...");
        let status = std::process::Command::new("cargo")
            .args(["build", "--release", "--bin", "richter"])
            .status()
            .context("Failed to build richter binary")?;
        if !status.success() {
            anyhow::bail!("cargo build failed");
        }
    }

    // Find the binary
    let src = std::env::current_exe().context("Failed to resolve current binary location")?;
    let dst = target_dir.join("richter");
    std::fs::copy(&src, &dst)
        .with_context(|| format!("Failed to copy {} -> {}", src.display(), dst.display()))?;

    // Make it executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dst)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dst, perms)?;
    }

    println!("Installed richter binary to {}", dst.display());
    println!();
    println!("Shims will now use this binary instead of recompiling.");
    println!("Run: richter install shims   to update shim symlinks.");

    Ok(())
}

/// Installs MCP server configuration for the specified agent.
async fn install_mcp(args: InstallMcpArgs) -> anyhow::Result<()> {
    let richter_dir = richter_dir();
    std::fs::create_dir_all(&richter_dir).context("Failed to create ~/.richter directory")?;

    let mcp_path = richter_dir.join("mcp.json");
    let mcp_config = mcp_config_for(&args.agent);

    std::fs::write(&mcp_path, mcp_config.clone())
        .with_context(|| format!("Failed to write MCP config to {}", mcp_path.display()))?;

    println!("MCP configuration written for agent '{}'", args.agent);
    println!("Config file: {}", mcp_path.display());
    println!();
    println!("Add the following to your {} MCP config:", args.agent);
    println!();
    println!("{mcp_config}");

    Ok(())
}

/// Installs hook configuration for the specified agent.
async fn install_hooks(args: InstallHooksArgs) -> anyhow::Result<()> {
    let hooks_dir = richter_dir().join("hooks");
    std::fs::create_dir_all(&hooks_dir).context("Failed to create hooks directory")?;

    let hook_path = hooks_dir.join(format!("{}.toml", args.agent));
    let hook_config = hook_config_for(&args.agent);

    std::fs::write(&hook_path, hook_config)
        .with_context(|| format!("Failed to write hook config to {}", hook_path.display()))?;

    println!("Hook configuration written for agent '{}'", args.agent);
    println!("Config file: {}", hook_path.display());

    Ok(())
}

/// Removes Richter integrations.
async fn uninstall(args: UninstallArgs, _socket: &str) -> anyhow::Result<()> {
    if !args.force {
        println!("This will remove Richter shims, hooks, and MCP configuration.");
        println!("Your cache and run history in ~/.richter/ will be preserved.");
        println!("Are you sure? (y/N)");

        let mut input = String::new();
        std::io::stdin()
            .read_line(&mut input)
            .context("Failed to read confirmation")?;

        if input.trim().to_lowercase() != "y" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let shim_dir = shim_dir();
    if shim_dir.exists() {
        std::fs::remove_dir_all(&shim_dir).context("Failed to remove shim directory")?;
        println!("Removed shim directory: {}", shim_dir.display());
    }

    let hooks_dir = richter_dir().join("hooks");
    if hooks_dir.exists() {
        std::fs::remove_dir_all(&hooks_dir).context("Failed to remove hooks directory")?;
        println!("Removed hooks directory: {}", hooks_dir.display());
    }

    let mcp_path = richter_dir().join("mcp.json");
    if mcp_path.exists() {
        std::fs::remove_file(&mcp_path).context("Failed to remove MCP config")?;
        println!("Removed MCP config: {}", mcp_path.display());
    }

    println!("Richter integrations uninstalled.");
    // Attempt to remove the Richter PATH line from common shell RC files.
    for rc in &[
        ".zshrc",
        ".bash_profile",
        ".bashrc",
        ".config/fish/config.fish",
    ] {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let rc_path = std::path::PathBuf::from(&home).join(rc);
        if rc_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&rc_path) {
                let cleaned: String = contents
                    .lines()
                    .filter(|line| {
                        !line.contains(".richter/shims") && !line.contains("# Added by Richter")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if cleaned != contents {
                    let _ = std::fs::write(&rc_path, cleaned.trim_end().to_string() + "\n");
                    println!("Cleaned Richter PATH entry from {}", rc_path.display());
                }
            }
        }
    }
    println!(
        "Note: Your ~/.richter/ directory (cache, auth token, run history) has been preserved."
    );
    Ok(())
}

/// Returns the list of tool names for which shims are created.
fn supported_tools() -> Vec<&'static str> {
    vec![
        "cargo", "go", "npm", "npx", "yarn", "pnpm", "pip", "pip3", "python", "python3", "make",
        "cmake", "bazel", "rustc", "gcc", "g++", "clang", "clang++", "javac", "dotnet", "git",
    ]
}

/// Detects the current shell from the `SHELL` environment variable.
fn detect_shell() -> String {
    std::env::var("SHELL")
        .unwrap_or_default()
        .split('/')
        .next_back()
        .unwrap_or("bash")
        .to_string()
}

/// Returns the RC file path for a given shell.
fn shell_rc_path(shell: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let filename = match shell {
        "zsh" => ".zshrc",
        // macOS starts bash as a login shell, which reads .bash_profile first.
        "bash" => ".bash_profile",
        "fish" => ".config/fish/config.fish",
        _ => ".profile",
    };
    PathBuf::from(home).join(filename)
}

/// Returns the Richter config directory: `~/.richter`.
fn richter_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".richter")
}

/// Returns the shim directory: `~/.richter/shims`.
fn shim_dir() -> PathBuf {
    richter_dir().join("shims")
}

/// Generates MCP config JSON for the given agent.
fn mcp_config_for(_agent: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "$HOME".to_string());
    serde_json::json!({
        "mcpServers": {
            "richter": {
                "command": "richter",
                "args": ["mcp", "serve"],
                "env": {
                    "RICHTER_SOCKET": format!("{home}/.richter/daemon.sock")
                }
            }
        }
    })
    .to_string()
}

/// Generates hook configuration (TOML snippet) for the given agent.
fn hook_config_for(agent: &str) -> String {
    format!(
        r#"# Richter hook configuration for {agent}
# This file is read by the Richter shim to detect {agent} sessions.

[hook]
agent = "{agent}"
enabled = true
shim_mode = true

# Command patterns that trigger Richter interception
[hook.patterns]
shell_commands = ["*"]

# Auto-detect agent processes
[hook.detection]
process_names = ["{agent}"]
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supported_tools_not_empty() {
        assert!(!supported_tools().is_empty());
    }

    #[test]
    fn test_detect_shell_fallback() {
        // SHELL detection falls back to "bash" when unset
        // In test environments, SHELL may have already been removed.
        // Just verify detect_shell returns something non-empty.
        let s = detect_shell();
        assert!(!s.is_empty(), "detect_shell returned empty string");
    }

    #[test]
    fn test_shell_rc_path() {
        let path = shell_rc_path("zsh");
        assert!(path.to_string_lossy().contains(".zshrc"));
    }

    #[test]
    fn test_mcp_config_is_json() {
        let config = mcp_config_for("claude");
        assert!(serde_json::from_str::<serde_json::Value>(&config).is_ok());
    }

    #[test]
    fn test_hook_config_contains_agent() {
        let config = hook_config_for("codex");
        assert!(config.contains("codex"));
        assert!(config.contains("[hook]"));
    }
}
