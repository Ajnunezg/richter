//! Configuration parsing for Richter.
//!
//! Reads global config from `~/.richter/config.toml` and per-repo config
//! from `.richter/config.toml` and `.richter/policy.yaml`. Provides typed
//! access to command policies, resource limits, cache TTLs, model providers,
//! notification thresholds, and redaction rules.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// Full Richter configuration, merging global and per-repo settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RichterConfig {
    /// List of watched workspace folders.
    #[serde(default)]
    pub watched_folders: Vec<PathBuf>,

    /// Command policies.
    #[serde(default)]
    pub commands: Vec<CommandPolicy>,

    /// Resource limits.
    #[serde(default)]
    pub resources: ResourceLimits,

    /// Cache TTLs per command class (in seconds).
    #[serde(default)]
    pub cache_ttls: HashMap<String, u64>,

    /// Model provider configuration.
    #[serde(default)]
    pub model_providers: Vec<ModelProviderConfig>,

    /// Notification thresholds.
    #[serde(default)]
    pub notifications: NotificationConfig,

    /// Redaction rules.
    #[serde(default)]
    pub redaction: RedactionConfig,

    /// Hook install choices.
    #[serde(default)]
    pub hooks: HooksConfig,

    /// Plugin manifests.
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,
    #[serde(default)]
    pub templates: Vec<TemplateConfig>,

    /// Data retention limits (in days).
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub parsers: Vec<ParserConfig>,

    /// General settings.
    #[serde(default)]
    pub general: GeneralConfig,

    /// Environment variable denylist for spawned processes.
    /// Defaults to dangerous keys like PATH, LD_PRELOAD, etc.
    #[serde(default = "default_env_denylist")]
    pub env_denylist: Vec<String>,
}

/// Default denied environment variable keys.
fn default_env_denylist() -> Vec<String> {
    vec![
        "PATH".into(),
        "LD_PRELOAD".into(),
        "LD_LIBRARY_PATH".into(),
        "DYLD_INSERT_LIBRARIES".into(),
        "DYLD_LIBRARY_PATH".into(),
    ]
}

impl Default for RichterConfig {
    fn default() -> Self {
        RichterConfig {
            watched_folders: vec![dirs_home()],
            commands: Vec::new(),
            resources: ResourceLimits::default(),
            cache_ttls: default_cache_ttls(),
            model_providers: Vec::new(),
            notifications: NotificationConfig::default(),
            redaction: RedactionConfig::default(),
            hooks: HooksConfig::default(),
            plugins: Vec::new(),
            templates: Vec::new(),
            retention: RetentionConfig::default(),
            parsers: Vec::new(),
            general: GeneralConfig::default(),
            env_denylist: default_env_denylist(),
        }
    }
}

// ---------------------------------------------------------------------------
// Command policy
// ---------------------------------------------------------------------------

/// A policy rule for a specific command or command pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandPolicy {
    /// A pattern to match the command (supports glob-like matching).
    pub r#match: String,

    /// The command class for this match.
    #[serde(default)]
    pub class: Option<String>,

    /// Whether to cache results for this command.
    #[serde(default)]
    pub cache: bool,

    /// Cache TTL in a human-readable format (e.g. "10m", "1h").
    pub ttl: Option<String>,

    /// Whether to deduplicate equivalent commands.
    #[serde(default = "default_true")]
    pub dedupe: bool,

    /// Resource lock key for this command (e.g. "node_modules", "target").
    pub resource_lock: Option<String>,

    /// Whether this command is marked as safe for deduplication.
    #[serde(default)]
    pub safe_for_dedupe: bool,

    /// Whether to pass through without management.
    #[serde(default)]
    pub passthrough: bool,

    /// Additional environment variables to include in fingerprints.
    #[serde(default)]
    pub fingerprint_env_vars: Vec<String>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Resource limits
// ---------------------------------------------------------------------------

/// Resource limits for the scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum number of heavy runs per repository.
    #[serde(default = "default_max_heavy_per_repo")]
    pub max_heavy_runs_per_repo: usize,

    /// Maximum number of heavy runs globally.
    #[serde(default = "default_max_heavy_global")]
    pub max_heavy_runs_global: usize,

    /// Maximum number of concurrent light runs per repo.
    #[serde(default = "default_max_light_per_repo")]
    pub max_light_runs_per_repo: usize,

    /// CPU pressure threshold (0.0-1.0) above which new heavy runs are queued.
    #[serde(default = "default_cpu_threshold")]
    pub cpu_pressure_threshold: f64,

    /// Memory pressure threshold (0.0-1.0) above which new runs are queued.
    #[serde(default = "default_memory_threshold")]
    pub memory_pressure_threshold: f64,
}

fn default_max_heavy_per_repo() -> usize {
    1
}
fn default_max_heavy_global() -> usize {
    3
}
fn default_max_light_per_repo() -> usize {
    4
}
fn default_cpu_threshold() -> f64 {
    0.85
}
fn default_memory_threshold() -> f64 {
    0.90
}

impl Default for ResourceLimits {
    fn default() -> Self {
        ResourceLimits {
            max_heavy_runs_per_repo: default_max_heavy_per_repo(),
            max_heavy_runs_global: default_max_heavy_global(),
            max_light_runs_per_repo: default_max_light_per_repo(),
            cpu_pressure_threshold: default_cpu_threshold(),
            memory_pressure_threshold: default_memory_threshold(),
        }
    }
}

// ---------------------------------------------------------------------------
// Model provider config
// ---------------------------------------------------------------------------

/// Secret string that redacts its value in Debug/Serialize output.
#[derive(Clone, Deserialize)]
pub struct SecretStringConfig {
    #[serde(default)]
    value: String,
}

impl std::fmt::Debug for SecretStringConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.value.is_empty() {
            write!(f, "SecretStringConfig(unset)")
        } else {
            write!(f, "SecretStringConfig([REDACTED])")
        }
    }
}

impl serde::Serialize for SecretStringConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

impl SecretStringConfig {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    pub fn expose_secret(&self) -> &str {
        &self.value
    }

    pub fn is_configured(&self) -> bool {
        !self.value.is_empty()
    }
}

#[allow(clippy::derivable_impls)]
impl Default for SecretStringConfig {
    fn default() -> Self {
        Self {
            value: String::new(),
        }
    }
}

/// Configuration for an LLM model provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProviderConfig {
    /// The provider name (e.g. "openai", "anthropic", "deepseek", "ollama").
    pub name: String,

    /// The model identifier.
    pub model: String,

    /// The purpose: "classification", "summarization", "adjudication".
    pub purpose: String,

    /// Whether this provider is the default for its purpose.
    #[serde(default)]
    pub default: bool,

    /// The API base URL, if not the standard one.
    pub api_base: Option<String>,

    /// Monthly budget in USD for this provider.
    pub budget_usd: Option<f64>,

    /// Maximum tokens per call.
    pub max_tokens: Option<u64>,

    /// API key for the provider.
    /// **Security**: redacted in Debug and never serialized.
    #[serde(default)]
    pub api_key: SecretStringConfig,

    /// Timeout for model calls (seconds).
    #[serde(default = "default_model_timeout")]
    pub timeout_secs: u64,

    /// Whether to use this provider for cheap-model boost.
    #[serde(default)]
    pub use_cheap: bool,

    /// Whether to use this provider for frontier-model boost.
    #[serde(default)]
    pub use_frontier: bool,
}

fn default_model_timeout() -> u64 {
    15
}

// ---------------------------------------------------------------------------
// Notification config
// ---------------------------------------------------------------------------

/// Notification and coalescing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Minimum importance level to trigger a macOS notification.
    #[serde(default = "default_notify_level")]
    pub min_notify_level: String,

    /// Coalesce events within this many seconds.
    #[serde(default = "default_coalesce_seconds")]
    pub coalesce_seconds: u64,

    /// Maximum notifications per minute.
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u64,

    /// Whether to show notifications for cache hits.
    #[serde(default)]
    pub notify_cache_hits: bool,
}

fn default_notify_level() -> String {
    "high".into()
}
fn default_coalesce_seconds() -> u64 {
    30
}
fn default_rate_limit() -> u64 {
    5
}

impl Default for NotificationConfig {
    fn default() -> Self {
        NotificationConfig {
            min_notify_level: default_notify_level(),
            coalesce_seconds: default_coalesce_seconds(),
            rate_limit_per_minute: default_rate_limit(),
            notify_cache_hits: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Redaction config
// ---------------------------------------------------------------------------

/// Redaction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionConfig {
    /// Whether redaction is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Additional regex patterns to redact.
    #[serde(default)]
    pub extra_patterns: Vec<String>,

    /// Patterns to exclude from redaction.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        RedactionConfig {
            enabled: true,
            extra_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Hooks config
// ---------------------------------------------------------------------------

/// Hook install configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Whether to install Claude Code hooks.
    #[serde(default)]
    pub claude: bool,
    /// Whether to install Codex hooks.
    #[serde(default)]
    pub codex: bool,
    /// Whether to install shell integration.
    #[serde(default)]
    pub shell: bool,
    /// Whether to install PATH shims.
    #[serde(default)]
    pub shims: bool,
}

impl Default for HooksConfig {
    fn default() -> Self {
        HooksConfig {
            claude: true,
            codex: true,
            shell: true,
            shims: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin config
// ---------------------------------------------------------------------------

/// Run template configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub name: String,
    pub steps: Vec<TemplateStep>,
    #[serde(default)]
    pub continue_on_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateStep {
    pub command: String,
    #[serde(default = "default_template_class")]
    pub class: String,
}
fn default_template_class() -> String {
    "unknown".into()
}

/// Plugin manifest configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// The plugin name.
    pub name: String,
    /// The plugin version.
    pub version: String,
    /// The agent type this plugin targets.
    pub agent_type: String,
    /// Whether the plugin is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Plugin-specific config.
    #[serde(default)]
    pub config: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Retention config
// ---------------------------------------------------------------------------

/// Custom parser DSL configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    pub name: String,
    pub match_output: String,
    pub extract_failures: Option<String>,
    pub extract_summary: Option<String>,
    #[serde(default = "default_importance")]
    pub importance: u8,
}
fn default_importance() -> u8 {
    75
}

/// Data retention configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Number of days to retain run history.
    #[serde(default = "default_retention_days")]
    pub run_history_days: u64,

    /// Number of days to retain raw logs.
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u64,

    /// Maximum number of events to retain.
    #[serde(default = "default_max_events")]
    pub max_events: u64,
}

fn default_retention_days() -> u64 {
    30
}
fn default_log_retention_days() -> u64 {
    7
}
fn default_max_events() -> u64 {
    100_000
}

impl Default for RetentionConfig {
    fn default() -> Self {
        RetentionConfig {
            run_history_days: default_retention_days(),
            log_retention_days: default_log_retention_days(),
            max_events: default_max_events(),
        }
    }
}

// ---------------------------------------------------------------------------
// General config
// ---------------------------------------------------------------------------

/// General settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Whether to start the daemon automatically.
    #[serde(default = "default_true")]
    pub auto_start_daemon: bool,

    /// Whether coordination is active.
    #[serde(default = "default_true")]
    pub coordination_active: bool,

    /// Maximum agent concurrency per repo.
    #[serde(default = "default_max_agents_per_repo")]
    pub max_agents_per_repo: usize,

    /// Whether to show the menu bar icon.
    #[serde(default = "default_true")]
    pub show_menu_bar: bool,
}

fn default_max_agents_per_repo() -> usize {
    10
}

impl Default for GeneralConfig {
    fn default() -> Self {
        GeneralConfig {
            auto_start_daemon: true,
            coordination_active: true,
            max_agents_per_repo: default_max_agents_per_repo(),
            show_menu_bar: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Load the global config from `~/.richter/config.toml`.
pub fn load_global_config() -> Result<RichterConfig> {
    let path = global_config_path();
    if path.exists() {
        load_config_file(&path)
    } else {
        Ok(RichterConfig::default())
    }
}

/// Load the per-repo config by merging `.richter/config.toml` over global defaults.
pub fn load_repo_config(repo_root: &Path) -> Result<RichterConfig> {
    let mut config = load_global_config()?;

    let repo_config_path = repo_root.join(".richter").join("config.toml");
    if repo_config_path.exists() {
        let repo_config: RichterConfig = load_config_file(&repo_config_path)?;
        merge_config(&mut config, &repo_config);
    }

    // Also check for policy overrides (TOML, with JSON fallback)
    let policy_path = repo_root.join(".richter").join("policy.toml");
    if policy_path.exists() {
        let policy_raw =
            std::fs::read_to_string(&policy_path).context("read .richter/policy.toml")?;
        let policies: Vec<CommandPolicy> = toml::from_str(&policy_raw)
            .or_else(|_| serde_json::from_str(&policy_raw))
            .with_context(|| format!("parse policy file {}", policy_path.display()))?;
        config.commands.extend(policies);
    }

    Ok(config)
}

/// Save the global config to `~/.richter/config.toml`.
pub fn save_global_config(config: &RichterConfig) -> Result<()> {
    let path = global_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create ~/.richter")?;
    }

    let toml_str = toml::to_string_pretty(config).context("serialize config to toml")?;
    std::fs::write(&path, toml_str).context("write config file")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn global_config_path() -> PathBuf {
    dirs_home().join(".richter").join("config.toml")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn load_config_file(path: &Path) -> Result<RichterConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read config file {}", path.display()))?;
    toml::from_str(&content).with_context(|| format!("parse config file {}", path.display()))
}

fn merge_config(base: &mut RichterConfig, overlay: &RichterConfig) {
    if !overlay.watched_folders.is_empty() {
        base.watched_folders = overlay.watched_folders.clone();
    }
    if !overlay.commands.is_empty() {
        // Repo commands take precedence; prepend them
        let mut merged = overlay.commands.clone();
        merged.append(&mut base.commands.clone());
        base.commands = merged;
    }
    // Resources: take non-default values from overlay
    if overlay.resources.max_heavy_runs_per_repo
        != ResourceLimits::default().max_heavy_runs_per_repo
    {
        base.resources.max_heavy_runs_per_repo = overlay.resources.max_heavy_runs_per_repo;
    }
    if overlay.resources.max_heavy_runs_global != ResourceLimits::default().max_heavy_runs_global {
        base.resources.max_heavy_runs_global = overlay.resources.max_heavy_runs_global;
    }
    if overlay.resources.max_light_runs_per_repo
        != ResourceLimits::default().max_light_runs_per_repo
    {
        base.resources.max_light_runs_per_repo = overlay.resources.max_light_runs_per_repo;
    }
    for (k, v) in &overlay.cache_ttls {
        base.cache_ttls.insert(k.clone(), *v);
    }
    if !overlay.model_providers.is_empty() {
        base.model_providers = overlay.model_providers.clone();
    }
    if overlay.redaction.enabled != RedactionConfig::default().enabled {
        base.redaction.enabled = overlay.redaction.enabled;
    }
    if !overlay.redaction.extra_patterns.is_empty() {
        base.redaction.extra_patterns = overlay.redaction.extra_patterns.clone();
    }
}

fn default_cache_ttls() -> HashMap<String, u64> {
    let mut ttls = HashMap::new();
    ttls.insert("build".into(), 1800); // 30 min
    ttls.insert("test".into(), 600); // 10 min
    ttls.insert("lint".into(), 300); // 5 min
    ttls.insert("typecheck".into(), 300); // 5 min
    ttls.insert("format".into(), 120); // 2 min
    ttls.insert("install".into(), 0); // Don't cache installs
    ttls.insert("dev_server".into(), 0); // Don't cache dev servers
    ttls.insert("migration".into(), 0); // Don't cache migrations
    ttls.insert("destructive".into(), 0); // Don't cache destructive
    ttls
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RichterConfig::default();
        assert_eq!(config.watched_folders.len(), 1);
        assert_eq!(config.resources.max_heavy_runs_per_repo, 1);
        assert_eq!(config.resources.max_heavy_runs_global, 3);
        assert!(config.redaction.enabled);
    }

    #[test]
    fn test_parse_minimal_toml() {
        let toml_str = r#"
watched_folders = ["/home/user/projects"]
"#;
        let config: RichterConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(config.watched_folders.len(), 1);
        assert_eq!(
            config.watched_folders[0],
            PathBuf::from("/home/user/projects")
        );
    }

    #[test]
    fn test_parse_full_config() {
        let toml_str = r#"
watched_folders = ["/home/user/projects", "/home/user/work"]

[[commands]]
match = "pnpm test"
class = "test"
cache = true
ttl = "10m"
dedupe = true

[[commands]]
match = "pnpm install"
class = "install"
cache = false
dedupe = false
resource_lock = "node_modules"

[resources]
max_heavy_runs_per_repo = 2
max_heavy_runs_global = 4

[cache_ttls]
build = 3600
test = 900
"#;
        let config: RichterConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(config.watched_folders.len(), 2);
        assert_eq!(config.commands.len(), 2);
        assert_eq!(config.commands[0].r#match, "pnpm test");
        assert!(config.commands[0].cache);
        assert_eq!(config.resources.max_heavy_runs_per_repo, 2);
        assert_eq!(config.cache_ttls.get("build"), Some(&3600));
    }

    #[test]
    fn test_parse_command_policy() {
        let toml_str = r#"
match = "cargo test"
class = "test"
cache = true
ttl = "5m"
"#;
        let policy: CommandPolicy = toml::from_str(toml_str).expect("parse");
        assert_eq!(policy.r#match, "cargo test");
        assert!(policy.cache);
        assert_eq!(policy.ttl.unwrap(), "5m");
    }

    #[test]
    fn test_merge_config() {
        let mut base = RichterConfig::default();
        let mut overlay = RichterConfig::default();
        overlay.resources.max_heavy_runs_per_repo = 5;
        overlay.commands.push(CommandPolicy {
            r#match: "cargo build".into(),
            class: Some("build".into()),
            cache: true,
            ttl: Some("30m".into()),
            dedupe: true,
            resource_lock: None,
            safe_for_dedupe: true,
            passthrough: false,
            fingerprint_env_vars: vec![],
        });

        merge_config(&mut base, &overlay);
        assert_eq!(base.resources.max_heavy_runs_per_repo, 5);
        assert_eq!(base.commands.len(), 1);
    }

    #[test]
    fn test_model_provider_config() {
        let toml_str = r#"
name = "openai"
model = "gpt-5.5"
purpose = "adjudication"
default = true
budget_usd = 10.0
max_tokens = 4096
"#;
        let provider: ModelProviderConfig = toml::from_str(toml_str).expect("parse");
        assert_eq!(provider.name, "openai");
        assert_eq!(provider.purpose, "adjudication");
        assert_eq!(provider.budget_usd, Some(10.0));
    }

    #[test]
    fn test_notification_defaults() {
        let config = NotificationConfig::default();
        assert_eq!(config.min_notify_level, "high");
        assert_eq!(config.coalesce_seconds, 30);
        assert_eq!(config.rate_limit_per_minute, 5);
    }

    #[test]
    fn test_retention_defaults() {
        let config = RetentionConfig::default();
        assert_eq!(config.run_history_days, 30);
        assert_eq!(config.log_retention_days, 7);
    }
}
