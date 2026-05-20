//! Richter daemon: background service that owns event ingestion, scheduling,
//! command deduplication, process supervision, local API, MCP server, and persistence.
//!
//! Runs as the current user via SMAppService/LoginItem. Survives UI app restarts.

pub mod api;
pub mod error;
pub mod event_bus;
pub mod importance;
pub mod mcp_bridge;
pub mod metrics;
pub mod mobile;
pub mod rate_limiter;
pub mod run_manager;
pub mod scheduler;
pub mod supervisor;
pub mod watcher;
pub mod webhooks;

pub mod plugin_runtime;
