//! Resource classification, pressure tracking, global daemon status,
//! model-call telemetry, settings, and API DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::event::ImportantEvent;
use super::ids::{ModelCallId, SettingId};

// ---------------------------------------------------------------------------
// ResourceClass
// ---------------------------------------------------------------------------

/// Resource class for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    /// Heavy build workload.
    HeavyBuild,
    /// Heavy test workload.
    HeavyTest,
    /// Light lint or type-check workload.
    LightLint,
    /// Dependency installation.
    Install,
    /// Long-running dev server.
    DevServer,
    /// Unknown workload class.
    Unknown,
}

impl std::fmt::Display for ResourceClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ResourceClass::HeavyBuild => "heavy_build",
            ResourceClass::HeavyTest => "heavy_test",
            ResourceClass::LightLint => "light_lint",
            ResourceClass::Install => "install",
            ResourceClass::DevServer => "dev_server",
            ResourceClass::Unknown => "unknown",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for ResourceClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "heavy_build" => Ok(ResourceClass::HeavyBuild),
            "heavy_test" => Ok(ResourceClass::HeavyTest),
            "light_lint" => Ok(ResourceClass::LightLint),
            "install" => Ok(ResourceClass::Install),
            "dev_server" => Ok(ResourceClass::DevServer),
            "unknown" => Ok(ResourceClass::Unknown),
            other => Err(format!("unknown ResourceClass: {other}")),
        }
    }
}

impl ResourceClass {
    /// Estimated CPU weight for this class (0.0 – 1.0).
    pub fn cpu_weight(&self) -> f64 {
        match self {
            ResourceClass::HeavyBuild => 0.85,
            ResourceClass::HeavyTest => 0.80,
            ResourceClass::LightLint => 0.40,
            ResourceClass::Install => 0.30,
            ResourceClass::DevServer => 0.15,
            ResourceClass::Unknown => 0.20,
        }
    }

    /// Whether this class can run concurrently with another instance of the same class.
    pub fn allows_concurrency(&self) -> bool {
        matches!(
            self,
            ResourceClass::LightLint | ResourceClass::DevServer | ResourceClass::Unknown
        )
    }
}

// ---------------------------------------------------------------------------
// DaemonSeverity
// ---------------------------------------------------------------------------

/// The severity of the daemon's global status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonSeverity {
    /// Everything is calm.
    Calm,
    /// There is activity but no problem.
    Active,
    /// Something needs attention.
    Warning,
    /// Immediate action required.
    Critical,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Resource pressure snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourcePressure {
    /// CPU usage (0.0-1.0).
    pub cpu: f64,
    /// Memory usage fraction (0.0-1.0).
    pub memory: f64,
    /// Number of active heavy builds.
    pub active_heavy_builds: usize,
    /// Number of active heavy tests.
    pub active_heavy_tests: usize,
    /// Total active processes under Richter.
    pub total_active_processes: usize,
}

/// A call to an external model (LLM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCall {
    /// Unique identifier.
    pub id: ModelCallId,
    /// The provider name.
    pub provider: String,
    /// The model name.
    pub model: String,
    /// The purpose of the call (classification, summarization, adjudication).
    pub purpose: String,
    /// The input token count.
    pub input_tokens: Option<u64>,
    /// The output token count.
    pub output_tokens: Option<u64>,
    /// The estimated cost in USD.
    pub cost_usd: Option<f64>,
    /// The response latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// When the call was made.
    pub called_at: DateTime<Utc>,
}

/// A key-value setting persisted by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setting {
    /// Unique identifier.
    pub id: SettingId,
    /// The setting key.
    pub key: String,
    /// The setting value (JSON-encoded).
    pub value: serde_json::Value,
    /// When the setting was last updated.
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Global status snapshot returned by `richter status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalStatus {
    /// Overall daemon severity.
    pub severity: DaemonSeverity,
    /// Number of tracked repositories.
    pub repo_count: usize,
    /// Number of tracked worktrees.
    pub worktree_count: usize,
    /// Number of detected agents.
    pub agent_count: usize,
    /// Number of active runs.
    pub active_runs: usize,
    /// Number of queued runs.
    pub queued_runs: usize,
    /// Number of cache hits today.
    pub cache_hits_today: u64,
    /// Number of duplicate runs avoided.
    pub duplicates_prevented: u64,
    /// Current CPU usage estimate (0.0-1.0).
    pub cpu_pressure: f64,
    /// Current memory pressure (0.0-1.0).
    pub memory_pressure: f64,
    /// The most important event, if any.
    pub top_event: Option<ImportantEvent>,
    /// Whether daemon coordination is active.
    pub coordination_active: bool,
    /// Whether shims are installed.
    pub shims_installed: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_status_defaults() {
        let status = GlobalStatus {
            severity: DaemonSeverity::Calm,
            repo_count: 0,
            worktree_count: 0,
            agent_count: 0,
            active_runs: 0,
            queued_runs: 0,
            cache_hits_today: 0,
            duplicates_prevented: 0,
            cpu_pressure: 0.0,
            memory_pressure: 0.0,
            top_event: None,
            coordination_active: true,
            shims_installed: false,
        };
        assert_eq!(status.severity, DaemonSeverity::Calm);
    }

    #[test]
    fn test_resource_pressure_bounds() {
        let rp = ResourcePressure {
            cpu: 0.75,
            memory: 0.60,
            active_heavy_builds: 2,
            active_heavy_tests: 1,
            total_active_processes: 8,
        };
        assert!(rp.cpu >= 0.0 && rp.cpu <= 1.0);
        assert!(rp.memory >= 0.0 && rp.memory <= 1.0);
    }
}
