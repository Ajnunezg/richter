//! Plugin runtime loader for Richter.
//! Discovers plugins from ~/.richter/plugins/ via --richter-manifest flag.

use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub version: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub capabilities: Vec<String>,
}

pub struct PluginRuntime {
    plugins: Vec<Plugin>,
    plugin_dir: PathBuf,
}

impl PluginRuntime {
    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Self { plugins: Vec::new(), plugin_dir: PathBuf::from(home).join(".richter").join("plugins") }
    }

    pub fn discover(&mut self) -> anyhow::Result<usize> {
        if !self.plugin_dir.exists() { return Ok(0); }
        let mut count = 0;
        for entry in std::fs::read_dir(&self.plugin_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Ok(output) = Command::new(&path).arg("--richter-manifest").output() {
                    if output.status.success() {
                        if let Ok(m) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                            self.plugins.push(Plugin {
                                name: m["name"].as_str().unwrap_or("unknown").into(),
                                version: m["version"].as_str().unwrap_or("0.1.0").into(),
                                path: path.clone(),
                                enabled: m["enabled"].as_bool().unwrap_or(true),
                                capabilities: m["capabilities"].as_array()
                                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                    .unwrap_or_default(),
                            });
                            count += 1;
                        }
                    }
                }
            }
        }
        tracing::info!("Discovered {count} plugin(s)");
        Ok(count)
    }

    pub fn list(&self) -> &[Plugin] { &self.plugins }
    pub fn has_plugin(&self, name: &str) -> bool { self.plugins.iter().any(|p| p.name == name && p.enabled) }
}

impl Default for PluginRuntime { fn default() -> Self { Self::new() } }
