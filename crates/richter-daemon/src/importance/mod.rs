//! Important-event engine for the Richter daemon.
//!
//! Provides deterministic parsers for popular test/output formats and an
//! importance scoring pipeline: deterministic filters → optional cheap model
//! → optional frontier model.
//!
//! Notification policy: coalesce, rate-limit, only high/crit in notifications.

pub mod llm;
pub mod parsers;
pub mod pipeline;

pub use llm::HTTPModelBoost;
pub use parsers::{
    detect_parser_from_command, BazelParser, CargoParser, EslintParser, GoTestParser, JunitParser,
    OutputParser, ParseResult, PytestParser, TapParser, TscParser, TurboNxParser, XcodebuildParser,
};
pub use pipeline::{
    ImportanceConfig, ImportanceEngine, ModelBoostProvider, NoopModelBoost, ShellModelBoost,
};

/// Importance severity level.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum Severity {
    /// Informational; no action needed.
    Low,
    /// Something to note; may need attention.
    Medium,
    /// Something broke; should be addressed soon.
    High,
    /// Urgent breakage; fix immediately.
    Critical,
}

impl Severity {
    /// Derive severity from exit code and failure count.
    pub fn from_exit_code_and_count(exit_code: i32, failure_count: usize) -> Self {
        if exit_code == 0 {
            Severity::Low
        } else {
            match failure_count {
                0 => Severity::Low,
                1..=3 => Severity::High,
                _ => Severity::Critical,
            }
        }
    }
}
