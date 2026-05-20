//! Importance scoring pipeline.
//!
//! Orchestrates the analysis pipeline: deterministic parsing → optional cheap-model
//! boost → optional frontier-model boost → event emission → notification dispatch.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::process::Command;
use std::time::Duration;
use tracing::info;

use super::llm::HTTPModelBoost;
use super::parsers::{
    BazelParser, CargoParser, EslintParser, GoTestParser, JunitParser, OutputParser, ParseResult,
    PytestParser, TapParser, TscParser, TurboNxParser, XcodebuildParser,
};
use super::Severity;
use crate::event_bus::{DaemonEvent, EventBus};

/// A provider that can boost severity using an external model (cheap or frontier).
/// Implementations may call local LLMs, HTTP endpoints, or shell commands.
#[async_trait]
pub trait ModelBoostProvider: Send + Sync {
    /// Boost the severity of an event using model inference.
    /// Returns the boosted severity, or the original if the model is unavailable.
    async fn boost(&self, severity: Severity, result: &ParseResult) -> Severity;

    /// Human-readable name for logging.
    fn name(&self) -> &'static str;
}

/// A no-op model boost provider that returns severity unchanged.
/// Used when model integration is not configured.
pub struct NoopModelBoost;

#[async_trait]
impl ModelBoostProvider for NoopModelBoost {
    async fn boost(&self, severity: Severity, _result: &ParseResult) -> Severity {
        severity
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}

/// A model boost provider that calls an external command.
///
/// The command receives the severity and parse result as JSON on stdin,
/// and returns a severity string on stdout.
///
/// Configure via `RICHTER_MODEL_BOOST_COMMAND` environment variable.
pub struct ShellModelBoost {
    command: String,
    args: Vec<String>,
    timeout: Duration,
}

impl ShellModelBoost {
    /// Create a `ShellModelBoost` from the `RICHTER_MODEL_BOOST_COMMAND` environment variable.
    /// Returns `None` if the variable is not set or empty.
    pub fn from_env() -> Option<Self> {
        let cmd = std::env::var("RICHTER_MODEL_BOOST_COMMAND").ok()?;
        let parts: Vec<String> = shlex::split(&cmd)
            .unwrap_or_else(|| cmd.split_whitespace().map(String::from).collect());
        if parts.is_empty() {
            return None;
        }
        let command = parts[0].clone();
        let args = parts[1..].to_vec();
        Some(Self {
            command,
            args,
            timeout: Duration::from_secs(10),
        })
    }
}

#[async_trait]
impl ModelBoostProvider for ShellModelBoost {
    async fn boost(&self, severity: Severity, result: &ParseResult) -> Severity {
        let input = serde_json::json!({
            "severity": format!("{:?}", severity),
            "failure_count": result.failure_count,
            "reason": result.reason,
        });

        let command = self.command.clone();
        let args = self.args.clone();
        let _timeout = self.timeout;
        let input_str = input.to_string();

        let output = tokio::task::spawn_blocking(move || {
            Command::new(&command)
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .ok()
                .and_then(|mut child| {
                    use std::io::Write;
                    if let Some(stdin) = child.stdin.as_mut() {
                        let _ = stdin.write_all(input_str.as_bytes());
                    }
                    drop(child.stdin.take());
                    child.wait_with_output().ok()
                })
        })
        .await;

        let output = match output {
            Ok(Some(o)) if o.status.success() => o,
            _ => {
                tracing::debug!(
                    "ShellModelBoost command failed or timed out, keeping original severity"
                );
                return severity;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_lowercase();
        match stdout.as_str() {
            "low" => Severity::Low,
            "medium" => Severity::Medium,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => severity,
        }
    }

    fn name(&self) -> &'static str {
        "shell"
    }
}

/// Configuration for the importance pipeline.
#[derive(Debug, Clone)]
pub struct ImportanceConfig {
    /// Whether to use a cheap LLM model for secondary scoring.
    pub use_cheap_model: bool,
    /// Whether to use a frontier LLM model for final scoring.
    pub use_frontier_model: bool,
    /// Minimum severity for notification delivery.
    pub min_notify_severity: Severity,
    /// Coalescence window for duplicate important events.
    pub coalesce_window: Duration,
    /// Maximum notifications per minute.
    pub max_notifications_per_min: usize,
}

impl Default for ImportanceConfig {
    fn default() -> Self {
        Self {
            use_cheap_model: false,
            use_frontier_model: false,
            min_notify_severity: Severity::High,
            coalesce_window: Duration::from_secs(30),
            max_notifications_per_min: 3,
        }
    }
}

/// The importance scoring engine.
pub struct ImportanceEngine {
    /// All registered parsers.
    parsers: Vec<Box<dyn OutputParser>>,
    /// Event bus for emitting important events.
    event_bus: EventBus,
    /// Configuration.
    config: ImportanceConfig,
    /// Cheap model boost provider.
    cheap_boost: Box<dyn ModelBoostProvider>,
    /// Frontier model boost provider.
    frontier_boost: Box<dyn ModelBoostProvider>,
    /// Last notification timestamp for rate limiting.
    last_notification: parking_lot::Mutex<Option<DateTime<Utc>>>,
    /// Notification count in the current window.
    notification_count: parking_lot::Mutex<usize>,
}

impl ImportanceEngine {
    /// Create a new importance engine with all built-in parsers.
    ///
    /// Model boost providers are selected via a priority chain:
    /// 1. `HTTPModelBoost::from_env()` if `RICHTER_LLM_ENDPOINT` is set.
    /// 2. `ShellModelBoost::from_env()` if `RICHTER_MODEL_BOOST_COMMAND` is set.
    /// 3. `NoopModelBoost` as the final fallback.
    pub fn new(config: ImportanceConfig, event_bus: EventBus) -> Self {
        let parsers: Vec<Box<dyn OutputParser>> = vec![
            Box::new(JunitParser::new()),
            Box::new(TapParser::new()),
            Box::new(CargoParser::new()),
            Box::new(PytestParser::new()),
            Box::new(XcodebuildParser::new()),
            Box::new(TscParser::new()),
            Box::new(EslintParser::new()),
            Box::new(GoTestParser::new()),
            Box::new(BazelParser::new()),
            Box::new(TurboNxParser::new()),
        ];

        let cheap_boost: Box<dyn ModelBoostProvider> = if let Some(http) =
            HTTPModelBoost::from_env()
        {
            info!(provider = %http.name(), "Importance engine using HTTP LLM boost provider");
            Box::new(http)
        } else if let Some(shell) = ShellModelBoost::from_env() {
            info!(provider = %shell.name(), "Importance engine using shell model boost provider");
            Box::new(shell)
        } else {
            info!(
                provider = "noop",
                "Importance engine using noop boost provider (no model configured)"
            );
            Box::new(NoopModelBoost)
        };

        let frontier_boost: Box<dyn ModelBoostProvider> = if let Some(http) =
            HTTPModelBoost::from_env()
        {
            info!(provider = %http.name(), "Importance engine using HTTP LLM frontier boost provider");
            Box::new(http)
        } else if let Some(shell) = ShellModelBoost::from_env() {
            info!(provider = %shell.name(), "Importance engine using shell model frontier boost provider");
            Box::new(shell)
        } else {
            Box::new(NoopModelBoost)
        };

        Self {
            parsers,
            event_bus,
            config,
            cheap_boost,
            frontier_boost,
            last_notification: parking_lot::Mutex::new(None),
            notification_count: parking_lot::Mutex::new(0),
        }
    }

    /// Returns the name of the currently active boost provider.
    pub fn provider_name(&self) -> &'static str {
        self.cheap_boost.name()
    }

    /// Set the cheap model boost provider.
    pub fn with_cheap_boost(mut self, provider: Box<dyn ModelBoostProvider>) -> Self {
        self.cheap_boost = provider;
        self
    }

    /// Set the frontier model boost provider.
    pub fn with_frontier_boost(mut self, provider: Box<dyn ModelBoostProvider>) -> Self {
        self.frontier_boost = provider;
        self
    }

    /// Analyze raw command output, classify severity, and emit important events.
    pub async fn analyze(
        &self,
        repo: &str,
        _command: &str,
        stdout: &str,
        exit_code: i32,
    ) -> Option<ParseResult> {
        let results: Vec<(&str, ParseResult)> = self
            .parsers
            .iter()
            .map(|p| (p.name(), p.parse(stdout, exit_code)))
            .filter(|(_, r)| r.failure_count > 0 || !r.reason.is_empty())
            .collect();

        if results.is_empty() {
            return None;
        }

        let best = results
            .iter()
            .max_by_key(|(_, r)| r.failure_count)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(ParseResult::success);

        if best.failure_count == 0 && exit_code == 0 {
            return None;
        }

        let severity = Severity::from_exit_code_and_count(exit_code, best.failure_count);

        let severity = if self.config.use_cheap_model {
            self.cheap_boost.boost(severity, &best).await
        } else {
            severity
        };

        let severity = if self.config.use_frontier_model {
            self.frontier_boost.boost(severity, &best).await
        } else {
            severity
        };

        let event = DaemonEvent::ImportantEvent {
            repo: repo.to_string(),
            severity: format!("{:?}", severity),
            reason: best.reason.clone(),
            details: serde_json::to_value(&best).unwrap_or_default(),
        };

        self.event_bus.emit(event);
        self.maybe_notify(severity, &best).await;

        Some(best)
    }

    /// Add a custom parser at runtime.
    pub fn add_parser(&mut self, parser: Box<dyn OutputParser>) {
        self.parsers.push(parser);
    }

    async fn maybe_notify(&self, severity: Severity, result: &ParseResult) {
        if severity < self.config.min_notify_severity {
            return;
        }

        let mut count = self.notification_count.lock();
        let mut last = self.last_notification.lock();

        let now = Utc::now();
        if let Some(prev) = *last {
            let elapsed = (now - prev).to_std().unwrap_or(Duration::MAX);
            if elapsed < self.config.coalesce_window {
                if *count >= self.config.max_notifications_per_min {
                    return;
                }
            } else {
                *count = 0;
            }
        }

        *count += 1;
        *last = Some(now);

        info!("IMPORTANT [{severity:?}]: {reason}", reason = result.reason);
    }
}
