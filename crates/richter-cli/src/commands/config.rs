//! `richter config` — manage Richter configuration.
//!
//! Supports get, set, list, and reload operations on daemon settings.

use crate::client::LocalClient;
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Show current configuration
    Show,
    /// Get a specific setting
    Get(ConfigGetArgs),
    /// Set a configuration value
    Set(ConfigSetArgs),
    /// Reload configuration from disk (hot reload)
    Reload,
}

#[derive(Args)]
pub struct ConfigGetArgs {
    /// Setting key to retrieve.
    pub key: String,
}

#[derive(Args)]
pub struct ConfigSetArgs {
    /// Setting key.
    pub key: String,
    /// Setting value.
    pub value: String,
}

pub async fn run(cmd: ConfigCommand, socket: &str) -> anyhow::Result<()> {
    match cmd {
        ConfigCommand::Show => show_config(socket).await,
        ConfigCommand::Get(args) => get_config(socket, &args.key).await,
        ConfigCommand::Set(args) => set_config(socket, &args.key, &args.value).await,
        ConfigCommand::Reload => reload_config(socket).await,
    }
}

async fn show_config(socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let req = serde_json::json!({"method": "settings"});
    let raw = client.send_raw(&req.to_string())?;
    let settings: serde_json::Value = serde_json::from_slice(&raw)?;
    println!("{}", serde_json::to_string_pretty(&settings)?);
    Ok(())
}

async fn get_config(socket: &str, key: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let req = serde_json::json!({"method": "settings"});
    let raw = client.send_raw(&req.to_string())?;
    let settings: serde_json::Value = serde_json::from_slice(&raw)?;
    match settings.get(key) {
        Some(v) => println!("{} = {}", key, v),
        None => println!("{} is not set", key),
    }
    Ok(())
}

async fn set_config(socket: &str, key: &str, value: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let v: serde_json::Value = serde_json::from_str(value)
        .unwrap_or(serde_json::Value::String(value.to_string()));
    let req = serde_json::json!({
        "method": "settings_update",
        "params": { "settings": { key: v } }
    });
    let raw = client.send_raw(&req.to_string())?;
    let resp: serde_json::Value = serde_json::from_slice(&raw)?;
    println!("{}", serde_json::to_string_pretty(&resp)?);
    Ok(())
}

async fn reload_config(socket: &str) -> anyhow::Result<()> {
    let client = LocalClient::new(socket);
    let req = serde_json::json!({"method": "settings_reload"});
    let raw = client.send_raw(&req.to_string())?;
    let resp: serde_json::Value = serde_json::from_slice(&raw)?;
    println!("{}", serde_json::to_string_pretty(&resp)?);
    Ok(())
}
