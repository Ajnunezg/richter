//! `richter doctor` — diagnostic report for the Richter system.
//!
//! Checks daemon connectivity, shim installation, shell PATH integration,
//! agent hook configuration, MCP server config, filesystem permissions,
//! model provider status, and overall system health.

use crate::client::LocalClient;
use anyhow::Context;
use clap::Args;

/// Arguments for the `doctor` subcommand.
#[derive(Args)]
pub struct DoctorArgs {
    /// Output report as JSON
    #[arg(long)]
    pub json: bool,

    /// Skip daemon connectivity check
    #[arg(long)]
    pub no_daemon: bool,
}

/// The result of a single diagnostic check.
#[derive(serde::Serialize)]
struct CheckResult {
    status: String,
    name: String,
    detail: String,
}

impl CheckResult {
    fn new(status: &str, name: &str, detail: impl Into<String>) -> Self {
        Self {
            status: status.to_string(),
            name: name.to_string(),
            detail: detail.into(),
        }
    }
}

/// Runs the doctor diagnostic suite.
pub async fn run(args: DoctorArgs, socket: &str) -> anyhow::Result<()> {
    let mut results: Vec<CheckResult> = Vec::new();

    // Daemon connectivity
    if !args.no_daemon {
        let reachable = LocalClient::new(socket).check_health().is_ok();
        results.push(CheckResult::new(
            if reachable { "ok" } else { "error" },
            "daemon",
            if reachable {
                format!("Reachable at {}", socket)
            } else {
                format!("Unreachable at {}. Is `richter-daemon` running?", socket)
            },
        ));
    }

    // Shim directory
    let shim_dir = shelldir::shim_dir();
    let shims_exist = shim_dir.exists()
        && shim_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
    results.push(CheckResult::new(
        if shims_exist { "ok" } else { "warning" },
        "shims",
        if shims_exist {
            format!("Shims installed at {}", shim_dir.display())
        } else {
            format!(
                "No shims found at {}. Run `richter install shims`.",
                shim_dir.display()
            )
        },
    ));

    // PATH integration
    let path_ok = check_path_integration(&shim_dir);
    results.push(CheckResult::new(
        if path_ok { "ok" } else { "warning" },
        "shell-path",
        if path_ok {
            "Shim directory is in PATH".to_string()
        } else {
            format!(
                "Shim directory {} is not in PATH. Run `richter install shell`.",
                shim_dir.display()
            )
        },
    ));

    // Agent hook configuration
    check_hooks(&mut results);

    // MCP configuration
    let mcp_ok = check_mcp();
    results.push(CheckResult::new(
        if mcp_ok { "ok" } else { "info" },
        "mcp",
        if mcp_ok {
            "MCP server configured for Richter".to_string()
        } else {
            "MCP server not configured. Run `richter install mcp` for supported agents.".to_string()
        },
    ));

    // Permissions
    let perms_ok = check_permissions();
    results.push(CheckResult::new(
        if perms_ok { "ok" } else { "warning" },
        "permissions",
        if perms_ok {
            "Richter directories are writable".to_string()
        } else {
            "Some Richter directories are not writable. Check ~/.richter/ permissions.".to_string()
        },
    ));

    // Model provider status
    check_provider_status(&mut results);

    // Output
    if args.json {
        let json = serde_json::to_string_pretty(&results)
            .context("Failed to serialize diagnostic report")?;
        println!("{json}");
    } else {
        print_human_report(&results);
    }

    Ok(())
}

fn check_path_integration(shim_dir: &std::path::Path) -> bool {
    let path_var = std::env::var("PATH").unwrap_or_default();
    path_var
        .split(':')
        .any(|p| std::path::Path::new(p) == shim_dir)
}

fn check_hooks(results: &mut Vec<CheckResult>) {
    let agents = ["claude", "codex"];

    for agent in agents {
        let configured = hook_configured(agent);
        results.push(CheckResult::new(
            if configured { "ok" } else { "info" },
            &format!("hooks-{agent}"),
            if configured {
                format!("{agent} hooks configured")
            } else {
                format!(
                    "{agent} hooks not configured. Run `richter install hooks --agent {agent}`."
                )
            },
        ));
    }
}

fn hook_configured(agent: &str) -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return false,
    };
    let path = std::path::PathBuf::from(home)
        .join(".richter")
        .join("hooks")
        .join(format!("{agent}.toml"));
    path.exists()
}

fn check_mcp() -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return false,
    };
    let path = std::path::PathBuf::from(home)
        .join(".richter")
        .join("mcp.json");
    path.exists()
}

fn check_permissions() -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return false,
    };
    let base = std::path::PathBuf::from(home).join(".richter");
    if !base.exists() {
        return true; // Not created yet, not an error
    }
    // Check if we can write a temp file
    let test_file = base.join(".doctor_write_test");
    std::fs::write(&test_file, b"").is_ok()
}

fn check_provider_status(results: &mut Vec<CheckResult>) {
    let providers = ["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"];
    for var in providers {
        let present = std::env::var(var).ok().is_some_and(|v| !v.is_empty());
        let name = var.to_lowercase().replace("_api_key", "");
        results.push(CheckResult::new(
            if present { "ok" } else { "info" },
            &format!("provider-{name}"),
            if present {
                format!("{var} is set")
            } else {
                format!("{var} is not set")
            },
        ));
    }
}

fn print_human_report(results: &[CheckResult]) {
    println!("Richter Diagnostic Report");
    println!("========================\n");

    let mut ok = 0;
    let mut warn = 0;
    let mut err = 0;
    let mut info = 0;

    for r in results {
        let icon = match r.status.as_str() {
            "ok" => {
                ok += 1;
                "✅"
            }
            "warning" => {
                warn += 1;
                "⚠️ "
            }
            "error" => {
                err += 1;
                "❌"
            }
            _ => {
                info += 1;
                "ℹ️ "
            }
        };
        println!("  {icon} {:<20} {}", r.name, r.detail);
    }

    println!("\n---");
    println!(
        "{} ok, {} warnings, {} errors, {} info",
        ok, warn, err, info
    );

    if err > 0 {
        std::process::exit(1);
    }
}

/// Helpers for resolving the shim directory.
mod shelldir {
    use std::path::PathBuf;

    /// Returns the canonical shim directory: `~/.richter/shims`.
    pub fn shim_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".richter").join("shims")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_path_integration_present() {
        let tmp = std::env::temp_dir();
        let path = format!("{}:{}:/usr/bin", tmp.display(), tmp.join("extra").display());
        std::env::set_var("PATH", &path);
        assert!(check_path_integration(&tmp));
    }

    #[test]
    fn test_check_path_integration_absent() {
        std::env::set_var("PATH", "/usr/bin:/bin");
        let tmp = std::env::temp_dir().join("__nonexistent_richter_shim__");
        assert!(!check_path_integration(&tmp));
    }

    #[test]
    fn test_shim_dir_resolves() {
        let dir = shelldir::shim_dir();
        assert!(dir.ends_with(".richter/shims"));
    }

    #[test]
    fn test_hook_configured_false() {
        let _ = hook_configured("nonexistent_agent_xyz");
    }

    #[test]
    fn test_check_permissions_safe() {
        let _ = check_permissions();
    }

    #[test]
    fn test_check_result_new() {
        let r = CheckResult::new("ok", "test", "detail");
        assert_eq!(r.status, "ok");
        assert_eq!(r.name, "test");
        assert_eq!(r.detail, "detail");
    }
}
