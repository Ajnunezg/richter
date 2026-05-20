//! Application-level metrics for the Richter daemon API.
//!
//! Exposes atomic counters that are incremented by handlers and middleware,
//! and a `/metrics` endpoint that produces Prometheus-format text output.
//! No external Prometheus crate is needed — just atomic counters and simple
//! text formatting. This is deliberate: a few counters don't justify a heavy
//! dependency tree.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Application-level metrics with atomic counters.
#[derive(Debug)]
pub struct AppMetrics {
    /// Total runs started (new processes spawned).
    pub runs_started: AtomicU64,
    /// Total runs completed (exited with any code).
    pub runs_completed: AtomicU64,
    /// Cache hits served from the in-memory result cache.
    pub cache_hits: AtomicU64,
    /// Duplicate runs prevented (joined an existing run).
    pub duplicates_prevented: AtomicU64,
    /// Authentication failures (invalid token, wrong scope).
    pub auth_failures: AtomicU64,
    /// Total HTTP requests processed.
    pub requests_total: AtomicU64,
    /// Rate-limited requests rejected.
    pub rate_limited: AtomicU64,
    /// Runs rejected (destructive, path traversal, etc.).
    pub runs_rejected: AtomicU64,
}

impl Default for AppMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl AppMetrics {
    /// Create a new metrics instance with all counters at zero.
    pub fn new() -> Self {
        Self {
            runs_started: AtomicU64::new(0),
            runs_completed: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            duplicates_prevented: AtomicU64::new(0),
            auth_failures: AtomicU64::new(0),
            requests_total: AtomicU64::new(0),
            rate_limited: AtomicU64::new(0),
            runs_rejected: AtomicU64::new(0),
        }
    }

    /// Increment a named counter. Used by middleware and handlers.
    pub fn inc(&self, counter: &MetricCounter) {
        let atomic = match counter {
            MetricCounter::RunsStarted => &self.runs_started,
            MetricCounter::RunsCompleted => &self.runs_completed,
            MetricCounter::CacheHits => &self.cache_hits,
            MetricCounter::DuplicatesPrevented => &self.duplicates_prevented,
            MetricCounter::AuthFailures => &self.auth_failures,
            MetricCounter::RequestsTotal => &self.requests_total,
            MetricCounter::RateLimited => &self.rate_limited,
            MetricCounter::RunsRejected => &self.runs_rejected,
        };
        atomic.fetch_add(1, Ordering::Relaxed);
    }

    /// Render all counters in Prometheus exposition text format.
    ///
    /// ```text
    /// # HELP richter_runs_started Total runs started
    /// # TYPE richter_runs_started counter
    /// richter_runs_started 42
    /// ```
    pub fn to_prometheus(&self) -> String {
        let counters = [
            (
                "runs_started",
                "Total runs started",
                self.runs_started.load(Ordering::Relaxed),
            ),
            (
                "runs_completed",
                "Total runs completed",
                self.runs_completed.load(Ordering::Relaxed),
            ),
            (
                "cache_hits",
                "Cache hits served from in-memory cache",
                self.cache_hits.load(Ordering::Relaxed),
            ),
            (
                "duplicates_prevented",
                "Duplicate runs prevented via join",
                self.duplicates_prevented.load(Ordering::Relaxed),
            ),
            (
                "auth_failures",
                "Authentication failures",
                self.auth_failures.load(Ordering::Relaxed),
            ),
            (
                "requests_total",
                "Total HTTP requests processed",
                self.requests_total.load(Ordering::Relaxed),
            ),
            (
                "rate_limited",
                "Requests rejected by rate limiter",
                self.rate_limited.load(Ordering::Relaxed),
            ),
            (
                "runs_rejected",
                "Runs rejected (destructive, path, etc.)",
                self.runs_rejected.load(Ordering::Relaxed),
            ),
        ];

        let mut out = String::with_capacity(counters.len() * 120);
        for (name, help, value) in &counters {
            out.push_str(&format!("# HELP richter_{name} {help}\n"));
            out.push_str(&format!("# TYPE richter_{name} counter\n"));
            out.push_str(&format!("richter_{name} {value}\n"));
        }
        out
    }
}

/// Named counter identifiers for type-safe incrementing.
#[derive(Debug, Clone, Copy)]
pub enum MetricCounter {
    RunsStarted,
    RunsCompleted,
    CacheHits,
    DuplicatesPrevented,
    AuthFailures,
    RequestsTotal,
    RateLimited,
    RunsRejected,
}

/// Convenience function to create shared metrics.
pub fn metrics() -> Arc<AppMetrics> {
    Arc::new(AppMetrics::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prometheus_format() {
        let m = AppMetrics::new();
        m.runs_started.store(42, Ordering::Relaxed);
        m.cache_hits.store(7, Ordering::Relaxed);
        m.auth_failures.store(3, Ordering::Relaxed);

        let output = m.to_prometheus();
        assert!(output.contains("richter_runs_started 42"));
        assert!(output.contains("richter_cache_hits 7"));
        assert!(output.contains("richter_auth_failures 3"));
        assert!(output.contains("# TYPE richter_runs_started counter"));
        assert!(output.contains("# HELP richter_runs_started"));
    }

    #[test]
    fn test_inc_counter() {
        let m = AppMetrics::new();
        m.inc(&MetricCounter::RequestsTotal);
        m.inc(&MetricCounter::RequestsTotal);
        assert_eq!(m.requests_total.load(Ordering::Relaxed), 2);

        m.inc(&MetricCounter::AuthFailures);
        assert_eq!(m.auth_failures.load(Ordering::Relaxed), 1);
    }
}
