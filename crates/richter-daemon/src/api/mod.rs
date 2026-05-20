//! Local API server for the Richter daemon.
//!
//! Binds to a Unix domain socket and exposes REST endpoints for the Agnos UI
//! and CLI to interact with the daemon. Uses axum with tower-http middleware.
//! All endpoints require a random auth token stored at 0600 permissions.

pub mod auth;
pub mod handlers;
pub mod middleware;

// Re-export public API from submodules.
pub use auth::{generate_auth_token, verify_restrictive_permissions};

use anyhow::Context;
use axum::{
    http::{header, Method, StatusCode},
    routing::get,
    Router,
};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixListener;
use tower_http::cors::CorsLayer;
use tower_http::timeout::TimeoutLayer;
use tracing::info;

use crate::metrics::AppMetrics;
use crate::mobile::MobileGatewayState;
use crate::rate_limiter::RateLimiter;

// ---------------------------------------------------------------------------
// Application state (decomposed from god object)
// ---------------------------------------------------------------------------

/// Budget tracking for model calls (circuit breaker).
#[derive(Debug, Clone)]
pub struct ModelCallBudget {
    /// Maximum calls per minute.
    pub max_calls_per_minute: u32,
    /// Calls made in the current window.
    pub calls_this_window: u32,
    /// Start of the current window.
    pub window_start: std::time::Instant,
}

impl Default for ModelCallBudget {
    fn default() -> Self {
        Self {
            max_calls_per_minute: 60,
            calls_this_window: 0,
            window_start: std::time::Instant::now(),
        }
    }
}

impl ModelCallBudget {
    /// Try to consume one call from the budget. Returns true if the call was allowed.
    pub fn try_consume(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start) >= std::time::Duration::from_secs(60) {
            self.calls_this_window = 0;
            self.window_start = now;
        }
        if self.calls_this_window < self.max_calls_per_minute {
            self.calls_this_window += 1;
            true
        } else {
            false
        }
    }

    /// Remaining calls in the current window.
    pub fn remaining(&self) -> u32 {
        self.max_calls_per_minute
            .saturating_sub(self.calls_this_window)
    }
}

/// Facilities for run management endpoints.
pub struct RunState {
    /// Scheduler for resource-gated command execution.
    pub scheduler: Arc<crate::scheduler::Scheduler>,
    /// Process supervisor for managing child processes.
    pub supervisor: Arc<crate::supervisor::ProcessSupervisor>,
    /// Run-or-join manager for deduplication.
    pub run_manager: Arc<crate::run_manager::RunManager>,
}

/// Facilities for system health and configuration endpoints.
pub struct SystemState {
    /// Database for persistence.
    pub db: Arc<richter_core::db::Database>,
    /// Event bus for pub/sub.
    pub event_bus: crate::event_bus::EventBus,
    /// Settings map.
    pub settings: Arc<ParkingMutex<HashMap<String, serde_json::Value>>>,
    /// Installation status.
    pub install_status: Arc<ParkingMutex<crate::api::InstallStatus>>,
    /// Watcher health flag.
    pub watcher_healthy: Arc<std::sync::atomic::AtomicBool>,
}

/// Facilities for authentication.
pub struct AuthState {
    /// Path to the auth token file.
    pub token_path: PathBuf,
    /// Cached auth token (loaded once at startup).
    pub auth_token: Arc<std::sync::OnceLock<String>>,
    /// Auth scope for the current token (read, write, admin).
    pub auth_scope: Arc<ParkingMutex<String>>,
}

/// Mobile gateway state (optional, only when mobile is enabled).
pub type MobileState = Option<Arc<MobileGatewayState>>;

/// Application state accessible from all API handlers.
pub struct AppState {
    /// Run management facilities.
    pub runs: RunState,
    /// System health and configuration facilities.
    pub system: SystemState,
    /// Authentication facilities.
    pub auth: AuthState,
    /// Mobile gateway state (optional).
    pub mobile: MobileState,
    /// Model call budget: max calls per minute.
    pub model_call_budget: Arc<parking_lot::Mutex<ModelCallBudget>>,
    /// Registered repositories.
    pub repos: Arc<ParkingMutex<Vec<RepoEntry>>>,
    /// Rate limiter for API requests.
    pub rate_limiter: Arc<RateLimiter>,
    /// Application-level metrics counters.
    pub metrics: Arc<AppMetrics>,
}

/// A registered repository entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Repository name.
    pub name: String,
    /// Absolute path.
    pub path: String,
    /// Whether the repo is currently being watched.
    pub watched: bool,
    /// Whether the working tree has uncommitted changes.
    pub dirty: bool,
}

/// Daemon installation / setup status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallStatus {
    /// Whether the daemon is installed as a login item.
    pub installed: bool,
    /// Whether the login item is registered.
    pub login_item_registered: bool,
    /// Whether the daemon is currently running.
    pub running: bool,
    /// Installed version string.
    pub version: String,
}

impl Default for InstallStatus {
    fn default() -> Self {
        Self {
            installed: false,
            login_item_registered: false,
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Health check response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Always "ok" when running.
    pub status: String,
    /// UTC timestamp.
    pub timestamp: String,
    /// Daemon version.
    pub version: String,
    /// Component health status.
    pub components: serde_json::Value,
}

/// Full status response.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    /// Health status.
    pub health: String,
    /// Number of active runs.
    pub active_runs: usize,
    /// Queue depth.
    pub queued_runs: usize,
    /// CPU percentage.
    pub cpu_percent: f32,
    /// Memory percentage.
    pub memory_percent: f32,
    /// Number of connected event subscribers.
    pub subscriber_count: usize,
    /// Cache hits today.
    pub cache_hits_today: u64,
    /// Duplicate runs prevented.
    pub duplicates_prevented: u64,
}

/// Structured metrics response for the `/metrics` JSON endpoint.
#[derive(Debug, Serialize)]
pub struct MetricsResponse {
    /// Number of currently active runs.
    pub active_runs: usize,
    /// Number of runs waiting in the scheduler queue.
    pub queued_runs: usize,
    /// Cache hits today.
    pub cache_hits_today: u64,
    /// Duplicate runs prevented.
    pub duplicates_prevented: u64,
    /// Scheduler permits available.
    pub scheduler_permits_available: usize,
    /// Scheduler queue depth.
    pub scheduler_queue_depth: usize,
}

/// Agent info returned by /agents.
#[derive(Debug, Serialize)]
pub struct AgentInfo {
    /// Agent identifier.
    pub id: String,
    /// Agent name.
    pub name: String,
    /// Current status.
    pub status: String,
}

/// Request body for /run_or_join.
#[derive(Debug, Deserialize)]
pub struct RunOrJoinRequest {
    /// Shell command to execute.
    pub command: String,
    /// Repository path (defaults to ".").
    #[serde(default = "default_repo")]
    pub repo: String,
    /// Environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Classification tag.
    #[serde(default = "default_classification")]
    pub classification: String,
    /// Resource class override.
    #[serde(default = "default_resource_class")]
    pub resource_class: String,
    /// Skip destructive preview gate.
    #[serde(default)]
    pub force: bool,
    /// Dry-run preview mode.
    #[serde(default)]
    pub preview: bool,
}

impl RunOrJoinRequest {
    /// Validate request fields before passing to the run manager.
    pub fn validate(&self) -> Result<(), String> {
        const MAX_COMMAND_LENGTH: usize = 4096;
        const MAX_REPO_LENGTH: usize = 4096;
        const MAX_ENV_KEY_LENGTH: usize = 256;
        const MAX_ENV_VALUE_LENGTH: usize = 4096;
        const MAX_ENV_ENTRIES: usize = 100;
        const MAX_CLASS_LENGTH: usize = 64;
        const FORBIDDEN_CHARS: &[char] = &['\0', '\n', '\r'];

        if self.command.is_empty() {
            return Err("command cannot be empty".into());
        }
        if self.command.len() > MAX_COMMAND_LENGTH {
            return Err(format!(
                "command exceeds maximum length of {} bytes",
                MAX_COMMAND_LENGTH
            ));
        }
        if self.command.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
            return Err("command contains forbidden characters".into());
        }

        if self.repo.len() > MAX_REPO_LENGTH {
            return Err(format!(
                "repo exceeds maximum length of {} bytes",
                MAX_REPO_LENGTH
            ));
        }
        if self.repo.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
            return Err("repo contains forbidden characters".into());
        }

        if self.env.len() > MAX_ENV_ENTRIES {
            return Err(format!(
                "env exceeds maximum of {} entries",
                MAX_ENV_ENTRIES
            ));
        }
        for (k, v) in &self.env {
            if k.len() > MAX_ENV_KEY_LENGTH {
                return Err(format!(
                    "env key exceeds maximum length of {} bytes",
                    MAX_ENV_KEY_LENGTH
                ));
            }
            if v.len() > MAX_ENV_VALUE_LENGTH {
                return Err(format!(
                    "env value exceeds maximum length of {} bytes",
                    MAX_ENV_VALUE_LENGTH
                ));
            }
        }

        if self.classification.len() > MAX_CLASS_LENGTH {
            return Err(format!(
                "classification exceeds maximum length of {} bytes",
                MAX_CLASS_LENGTH
            ));
        }
        if self.resource_class.len() > MAX_CLASS_LENGTH {
            return Err(format!(
                "resource_class exceeds maximum length of {} bytes",
                MAX_CLASS_LENGTH
            ));
        }

        Ok(())
    }
}

fn default_repo() -> String {
    ".".to_string()
}
fn default_classification() -> String {
    "unknown".to_string()
}
fn default_resource_class() -> String {
    "light_lint".to_string()
}

/// Settings update request.
#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    /// Settings to update (partial merge).
    pub settings: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Server builder
// ---------------------------------------------------------------------------

/// Build the API router.
pub fn build_router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/health", get(handlers::health_handler))
        .route("/status", get(handlers::status_handler))
        .route("/repos", get(handlers::repos_handler))
        .route("/agents", get(handlers::agents_handler))
        .route("/runs", get(handlers::runs_handler))
        .route("/runs/{run_id}", get(handlers::run_detail_handler))
        .route("/runs/{run_id}/output", get(handlers::run_output_handler))
        .route(
            "/run_or_join",
            axum::routing::post(handlers::run_or_join_handler),
        )
        .route("/stream_run/{run_id}", get(handlers::stream_run_handler))
        .route("/events", get(handlers::events_handler))
        .route("/install_status", get(handlers::install_status_handler))
        .route("/explain/{run_id}", get(handlers::explain_handler))
        .route("/audit", get(handlers::audit_handler))
        .route("/preview", axum::routing::post(handlers::preview_handler))
        .route("/budget", get(handlers::budget_handler))
        .route("/metrics", get(handlers::metrics_handler))
        .route(
            "/metrics/prometheus",
            get(handlers::prometheus_metrics_handler),
        )
        .route("/openapi.json", get(handlers::openapi_handler))
        .route("/onboard", get(handlers::onboard_handler))
        .route(
            "/settings",
            get(handlers::settings_get_handler).put(handlers::settings_put_handler),
        );

    router
        .layer(
            CorsLayer::new()
                .allow_origin([
                    "http://localhost:3000".parse().unwrap(),
                    "http://localhost:5173".parse().unwrap(),
                    "http://127.0.0.1:3000".parse().unwrap(),
                    "http://127.0.0.1:5173".parse().unwrap(),
                ])
                .allow_methods([Method::GET, Method::POST, Method::PUT])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ))
        .layer(axum::middleware::from_fn(middleware::request_id_middleware))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .with_state(state.clone())
}

/// Bind to the Unix domain socket path and start serving.
///
/// Removes any stale socket file at `socket_path` before binding.
pub async fn serve(state: Arc<AppState>, socket_path: &std::path::Path) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path).with_context(|| {
            format!("Failed to remove stale socket at {}", socket_path.display())
        })?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("Failed to bind to Unix socket at {}", socket_path.display()))?;

    let app = build_router(state);

    info!("API server listening on {}", socket_path.display());

    axum::serve(listener, app.into_make_service())
        .await
        .context("API server error")?;

    Ok(())
}
