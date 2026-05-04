//! Importance scoring pipeline.
//!
//! Orchestrates the analysis pipeline: deterministic parsing → optional cheap-model
//! boost → optional frontier-model boost → event emission → notification dispatch.

use chrono::{DateTime, Utc};
use std::time::Duration;
use tracing::info;

use super::parsers::{
    BazelParser, CargoParser, EslintParser, GoTestParser, JunitParser, OutputParser, ParseResult,
    PytestParser, TapParser, TscParser, TurboNxParser, XcodebuildParser,
};
use super::Severity;
use crate::event_bus::{DaemonEvent, EventBus};

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
    /// Last notification timestamp for rate limiting.
    last_notification: parking_lot::Mutex<Option<DateTime<Utc>>>,
    /// Notification count in the current window.
    notification_count: parking_lot::Mutex<usize>,
}

impl ImportanceEngine {
    /// Create a new importance engine with all built-in parsers.
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

        Self {
            parsers,
            event_bus,
            config,
            last_notification: parking_lot::Mutex::new(None),
            notification_count: parking_lot::Mutex::new(0),
        }
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
            self.cheap_model_boost(severity, &best)
        } else {
            severity
        };

        let severity = if self.config.use_frontier_model {
            self.frontier_model_boost(severity, &best)
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

    fn cheap_model_boost(&self, severity: Severity, result: &ParseResult) -> Severity {
        if result.failure_count > 10 && severity < Severity::Critical {
            Severity::Critical
        } else {
            severity
        }
    }

    fn frontier_model_boost(&self, severity: Severity, _result: &ParseResult) -> Severity {
        severity
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
