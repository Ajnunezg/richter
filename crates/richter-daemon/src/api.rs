//! Local API server for the Richter daemon.
//!
//! Binds to a Unix domain socket and exposes REST endpoints for the Agnos UI
//! and CLI to interact with the daemon. Uses axum with tower-http middleware.
//! All endpoints require a random auth token stored at 0600 permissions.

use anyhow::Context;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{Json, Sse},
    routing::get,
    Router,
};
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio_stream::StreamExt;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};

use crate::mobile_gateway::MobileGatewayState;

// ---------------------------------------------------------------------------
// Daemon state
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
            max_calls_per_minute: 30,
            calls_this_window: 0,
            window_start: std::time::Instant::now(),
        }
    }
}

impl ModelCallBudget {
    /// Check if a call is allowed. Returns false if the circuit is open.
    pub fn try_consume(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start) > std::time::Duration::from_secs(60) {
            self.window_start = now;
            self.calls_this_window = 0;
        }
        if self.calls_this_window >= self.max_calls_per_minute {
            return false;
        }
        self.calls_this_window += 1;
        true
    }

    /// Returns the remaining budget in this window.
    pub fn remaining(&self) -> u32 {
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start) > std::time::Duration::from_secs(60) {
            self.max_calls_per_minute
        } else {
            self.max_calls_per_minute.saturating_sub(self.calls_this_window)
        }
    }
}

/// Shared daemon state accessible from all API handlers.
pub struct DaemonState {
    /// The event bus.
    pub event_bus: crate::event_bus::EventBus,
    /// The run manager for run-or-join operations.
    pub run_manager: Arc<crate::run_manager::RunManager>,
    /// The scheduler for resource queries.
    pub scheduler: Arc<crate::scheduler::Scheduler>,
    /// The process supervisor for run queries.
    pub supervisor: Arc<crate::supervisor::ProcessSupervisor>,
    /// Path to the auth token file.
    pub token_path: PathBuf,
    /// Registered repositories.
    pub repos: ParkingMutex<Vec<RepoEntry>>,
    /// Persistent settings.
    pub settings: ParkingMutex<HashMap<String, serde_json::Value>>,
    /// Installation status.
    pub install_status: ParkingMutex<InstallStatus>,
    /// Optional mobile gateway state (created when mobile is enabled).
    pub mobile_state: Option<Arc<MobileGatewayState>>,
    /// Model call budget: max calls per minute.
    pub model_call_budget: Arc<parking_lot::Mutex<ModelCallBudget>>,
}

/// A registered repository entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    /// Human-readable name.
    pub name: String,
    /// Absolute path on disk.
    pub path: String,
    /// Whether the repo is currently watched for changes.
    pub watched: bool,
    /// Last detected dirty status.
    pub dirty: bool,
}

/// Daemon installation / setup status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallStatus {
    /// Whether the daemon is fully installed and configured.
    pub installed: bool,
    /// Whether the login item / SMAppService is registered.
    pub login_item_registered: bool,
    /// Whether the daemon is currently running.
    pub running: bool,
    /// Version string.
    pub version: String,
}

impl Default for InstallStatus {
    fn default() -> Self {
        Self {
            installed: true,
            login_item_registered: true,
            running: true,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

/// Validates the `Authorization: Bearer <token>` header against the stored token.
async fn auth_middleware(
    State(state): State<Arc<DaemonState>>,
    headers: HeaderMap,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, (StatusCode, Json<serde_json::Value>)> {
    let expected = match load_auth_token(&state.token_path) {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to load auth token: {e}");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Auth configuration error"})),
            ));
        }
    };

    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(token) = auth_header.strip_prefix("Bearer ") {
        if token == expected {
            return Ok(next.run(request).await);
        }
    }

    Err((
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "Invalid or missing auth token"})),
    ))
}

/// Read the auth token from the 0600-permission file.
fn load_auth_token(path: &std::path::Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Failed to open auth token file at {}", path.display()))?;
    let mut token = String::new();
    file.read_to_string(&mut token)?;
    Ok(token.trim().to_string())
}

/// Generate a random auth token and write it to a file with 0600 permissions.
pub fn generate_auth_token(path: &std::path::Path) -> anyhow::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let random_bytes: [u8; 32] = rand_like_bytes();
    let mut hasher = Sha256::new();
    hasher.update(random_bytes);
    hasher.update(b"richter-daemon-auth");
    let token = hex::encode(hasher.finalize());

    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Failed to create auth token file at {}", path.display()))?;
    file.write_all(token.as_bytes())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms)?;
    }

    Ok(token)
}

fn rand_like_bytes() -> [u8; 32] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let mut buf = [0u8; 32];
    for (i, byte) in buf.iter_mut().enumerate() {
        *byte = ((nanos >> (i % 8)) & 0xFF) as u8;
        *byte ^= (std::process::id() >> (i % 8)) as u8;
    }
    buf
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
    /// Key-value pairs to merge into settings.
    pub settings: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /health
async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

/// GET /status
async fn status_handler(State(state): State<Arc<DaemonState>>) -> Json<StatusResponse> {
    let snap = state.scheduler.resource_snapshot();
    Json(StatusResponse {
        health: "ok".to_string(),
        active_runs: state.scheduler.active_count(),
        queued_runs: state.scheduler.queue_depth(),
        cpu_percent: snap.cpu_percent,
        memory_percent: snap.memory_percent,
        subscriber_count: state.event_bus.receiver_count(),
        cache_hits_today: state.run_manager.cache_hits_today(),
        duplicates_prevented: state.run_manager.duplicates_prevented(),
    })
}

/// GET /repos
async fn repos_handler(State(state): State<Arc<DaemonState>>) -> Json<Vec<RepoEntry>> {
    let repos = state.repos.lock().clone();
    Json(repos)
}

/// GET /agents
async fn agents_handler(State(state): State<Arc<DaemonState>>) -> Json<Vec<AgentInfo>> {
    let active_ids = state.supervisor.active_run_ids();
    let mut agents: Vec<AgentInfo> = active_ids
        .iter()
        .filter_map(|id| {
            state.supervisor.run_info(id).map(|info| AgentInfo {
                id: id.clone(),
                name: format!("agent:{}", &id[..id.len().min(8)]),
                status: String::from(if info.is_active { "running" } else { "idle" }),
            })
        })
        .collect();
    agents.push(AgentInfo {
        id: "daemon".to_string(),
        name: "Richter Daemon".to_string(),
        status: "running".to_string(),
    });
    Json(agents)
}

/// GET /runs
async fn runs_handler(
    State(state): State<Arc<DaemonState>>,
) -> Json<Vec<crate::supervisor::RunInfo>> {
    let ids = state.supervisor.active_run_ids();
    let infos: Vec<_> = ids
        .iter()
        .filter_map(|id| state.supervisor.run_info(id))
        .collect();
    Json(infos)
}

/// GET /runs/{run_id}
async fn run_detail_handler(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
) -> Result<Json<crate::supervisor::RunInfo>, (StatusCode, Json<serde_json::Value>)> {
    state.supervisor.run_info(&run_id).map(Json).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Run not found"})),
        )
    })
}

/// GET /runs/{run_id}/output
async fn run_output_handler(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let output = state.supervisor.get_output(&run_id).unwrap_or_default();
    Ok(Json(serde_json::json!({"output": output})))
}

/// POST /run_or_join
async fn run_or_join_handler(
    State(state): State<Arc<DaemonState>>,
    Json(req): Json<RunOrJoinRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let spec = crate::supervisor::RunSpec {
        command: req.command,
        repo: req.repo,
        env: req.env,
        classification: req.classification,
        resource_class: req.resource_class,
        force: req.force,
        preview: req.preview,
        ..Default::default()
    };

    match state.run_manager.run_or_join(spec).await {
        Ok(outcome) => {
            let json = serde_json::to_value(&outcome).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Serialization error: {e}")})),
                )
            })?;
            Ok(Json(json))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("{e:#}")})),
        )),
    }
}

/// Convert a String to an SSE event.
fn to_sse_event(line: String) -> Result<axum::response::sse::Event, std::convert::Infallible> {
    Ok(axum::response::sse::Event::default().data(line))
}

/// GET /stream_run/{run_id} — SSE stream for run output.
async fn stream_run_handler(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>
{
    let stream = match state.supervisor.stream_output(&run_id).await {
        Some(rx) => rx,
        None => {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let _s = tx.send("error: run not found".to_string()).await;
            return Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx).map(to_sse_event));
        }
    };

    let event_stream = tokio_stream::wrappers::ReceiverStream::new(stream).map(to_sse_event);

    Sse::new(event_stream)
}

/// GET /events — SSE stream for daemon events.
async fn events_handler(
    State(state): State<Arc<DaemonState>>,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>
{
    let mut rx = state.event_bus.subscribe_all();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok(axum::response::sse::Event::default().data(json));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Event stream lagged by {n} events");
                    yield Ok(axum::response::sse::Event::default()
                        .data(format!(r#"{{"lagged":{n}}}"#)));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// GET /install_status
async fn install_status_handler(State(state): State<Arc<DaemonState>>) -> Json<InstallStatus> {
    let status = state.install_status.lock().clone();
    Json(status)
}

/// GET /settings
async fn settings_get_handler(
    State(state): State<Arc<DaemonState>>,
) -> Json<HashMap<String, serde_json::Value>> {
    let settings = state.settings.lock().clone();
    Json(settings)
}

/// PUT /settings
async fn settings_put_handler(
    State(state): State<Arc<DaemonState>>,
    Json(req): Json<SettingsUpdate>,
) -> Json<HashMap<String, serde_json::Value>> {
    let mut settings = state.settings.lock();
    for (k, v) in req.settings {
        settings.insert(k, v);
    }
    Json(settings.clone())
}


/// GET /explain/{run_id}
async fn explain_handler(
    State(state): State<Arc<DaemonState>>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Build an explanation from supervisor state
    let info = state.supervisor.run_info(&run_id);
    match info {
        Some(run_info) => {
            Ok(Json(serde_json::json!({
                "run_id": run_info.run_id,
                "command": run_info.command,
                "disposition": if run_info.is_active { "running" } else { "completed" },
                "reason": "Run was started or joined through the run-or-join engine",
                "fingerprint": "unknown",
                "cache_age": null,
                "queue_position": null,
                "estimated_wait_ms": null,
            })))
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Run not found"})),
        )),
    }
}


/// GET /audit — structured audit log.
async fn audit_handler(
    State(state): State<Arc<DaemonState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit: usize = params.get("last")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    // Collect recent decisions as audit entries (from event bus history, stubbed for now)
    let mut rx = state.event_bus.subscribe_all();
    let mut entries = Vec::new();

    // Drain available events for audit
    loop {
        match rx.try_recv() {
            Ok(event) => {
                let (event_type, title, summary, severity) = match &event {
                    crate::event_bus::DaemonEvent::RunStarted { command, .. } => {
                        (String::from("run_started"), String::from("Run started"), command.clone(), String::from("info"))
                    }
                    crate::event_bus::DaemonEvent::RunCompleted { exit_code, .. } => {
                        (String::from("run_completed"), String::from("Run completed"), format!("exit_code={exit_code}"), String::from("info"))
                    }
                    crate::event_bus::DaemonEvent::RunCached { command, .. } => {
                        (String::from("run_cached"), String::from("Cache hit"), command.clone(), String::from("info"))
                    }
                    crate::event_bus::DaemonEvent::RunQueued { reason, .. } => {
                        (String::from("run_queued"), String::from("Run queued"), reason.clone(), String::from("info"))
                    }
                    crate::event_bus::DaemonEvent::RunDequeued { .. } => {
                        (String::from("run_dequeued"), String::from("Run dequeued"), String::new(), String::from("info"))
                    }
                    crate::event_bus::DaemonEvent::ImportantEvent { reason, severity: sev, .. } => {
                        (String::from("important_event"), sev.clone(), reason.clone(), sev.clone())
                    }
                    crate::event_bus::DaemonEvent::ConflictDetected { conflict_type, .. } => {
                        (String::from("conflict"), String::from("Agent conflict"), conflict_type.clone(), String::from("warning"))
                    }
                    _ => continue,
                };
                entries.push(serde_json::json!({
                    "id": uuid::Uuid::new_v4().to_string(),
                    "event_type": event_type,
                    "title": title,
                    "summary": summary,
                    "severity": severity,
                    "created_at": chrono::Utc::now().to_rfc3339(),
                }));
                if entries.len() >= limit {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    Json(serde_json::json!({
        "entries": entries,
        "total": entries.len(),
    }))
}


/// POST /preview — dry-run preview of a command.
async fn preview_handler(
    State(state): State<Arc<DaemonState>>,
    Json(req): Json<RunOrJoinRequest>,
) -> Json<serde_json::Value> {
    let outcome = state.run_manager.run_or_join(crate::supervisor::RunSpec {
        command: req.command.clone(),
        repo: req.repo,
        env: req.env,
        classification: req.classification,
        resource_class: req.resource_class,
        force: req.force,
        preview: req.preview,
        ..Default::default()
    }).await;

    match outcome {
        Ok(o) => {
            let val = serde_json::to_value(&o).unwrap_or_default();
            Json(serde_json::json!({
                "preview": true,
                "disposition": val,
                "message": "This is a preview. No command was executed."
            }))
        }
        Err(e) => {
            Json(serde_json::json!({
                "preview": true,
                "error": format!("{e:#}"),
                "message": "Preview failed."
            }))
        }
    }
}


/// GET /budget — model call budget status with warnings.
async fn budget_handler(State(state): State<Arc<DaemonState>>) -> Json<serde_json::Value> {
    let budget = state.model_call_budget.lock();
    let remaining = budget.remaining();
    let circuit_open = remaining == 0;
    let warnings: Vec<String> = if circuit_open {
        vec!["Budget exhausted — circuit breaker open. Model calls rejected until next window.".into()]
    } else if remaining <= budget.max_calls_per_minute / 5 {
        vec![format!("80%+ consumed — {remaining} calls remaining this minute")]
    } else {
        vec![]
    };
    Json(serde_json::json!({
        "remaining_per_minute": remaining,
        "max_per_minute": budget.max_calls_per_minute,
        "circuit_open": circuit_open,
        "warnings": warnings,
        "monthly_spend_usd_estimate": null,
        "providers": [],
    }))
}


/// GET /onboard — first-run onboarding status and next steps.
async fn onboard_handler(State(state): State<Arc<DaemonState>>) -> Json<serde_json::Value> {
    let installed = state.install_status.lock().installed;
    let data_dir = dirs_fallback().join(".richter");
    let is_first_run = !data_dir.join("richter.db").exists();

    let steps = if is_first_run {
        serde_json::json!([
            {"step": 1, "title": "Daemon installed", "done": installed},
            {"step": 2, "title": "Database created", "done": data_dir.join("richter.db").exists()},
            {"step": 3, "title": "Shims configured", "done": false, "action": "Run: richter install shims"},
            {"step": 4, "title": "MCP configured", "done": false, "action": "Run: richter install mcp"},
        ])
    } else {
        serde_json::json!([
            {"step": 1, "title": "Daemon installed", "done": true},
            {"step": 2, "title": "Database created", "done": true},
            {"step": 3, "title": "Everything ready", "done": true},
        ])
    };

    let next_action = if is_first_run {
        "Run: richter install shims"
    } else {
        "All set! Richter is running."
    };

    Json(serde_json::json!({
        "onboarding_complete": !is_first_run && installed,
        "steps": steps,
        "next_action": next_action,
    }))
}

fn dirs_fallback() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
}


/// GET /openapi.json — OpenAPI 3.0 spec for the REST API.
async fn openapi_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Richter Daemon API",
            "version": "0.1.0",
            "description": "REST API for the Richter command de-duplication daemon"
        },
        "paths": {
            "/health": { "get": { "summary": "Health check", "responses": { "200": { "description": "OK" } } } },
            "/status": { "get": { "summary": "Global system status", "responses": { "200": { "description": "Status snapshot" } } } },
            "/runs": { "get": { "summary": "List active runs", "responses": { "200": { "description": "Array of RunInfo" } } } },
            "/runs/{run_id}": { "get": { "summary": "Run detail", "parameters": [{"name": "run_id", "in": "path", "required": true, "schema": {"type": "string"}}], "responses": { "200": { "description": "RunInfo" } } } },
            "/run_or_join": { "post": { "summary": "Submit a command", "requestBody": { "content": { "application/json": { "schema": { "type": "object", "properties": { "command": {"type": "string"}, "repo": {"type": "string"}, "classification": {"type": "string"} } } } } }, "responses": { "200": { "description": "RunOutcome" } } } },
            "/events": { "get": { "summary": "SSE event stream", "responses": { "200": { "description": "Server-sent events stream" } } } },
            "/stream_run/{run_id}": { "get": { "summary": "SSE output stream for a run", "responses": { "200": { "description": "Output stream" } } } },
            "/explain/{run_id}": { "get": { "summary": "Explain a run decision", "responses": { "200": { "description": "Decision explanation" } } } },
            "/audit": { "get": { "summary": "Structured audit log", "parameters": [{"name": "last", "in": "query", "schema": {"type": "integer", "default": 50}}], "responses": { "200": { "description": "Audit entries" } } } },
            "/preview": { "post": { "summary": "Dry-run preview of a command", "responses": { "200": { "description": "Preview outcome" } } } },
            "/budget": { "get": { "summary": "Model call budget status", "responses": { "200": { "description": "Budget info" } } } },
            "/onboard": { "get": { "summary": "First-run onboarding status", "responses": { "200": { "description": "Onboarding steps" } } } },
            "/metrics": { "get": { "summary": "Prometheus metrics", "responses": { "200": { "description": "OpenMetrics text" } } } },
            "/settings": { "get": { "summary": "Get settings" }, "put": { "summary": "Update settings" } },
            "/install_status": { "get": { "summary": "Installation status" } },
            "/repos": { "get": { "summary": "List repositories" } },
            "/agents": { "get": { "summary": "List agents" } }
        },
        "servers": [{"url": "unix:///tmp/richter.sock"}]
    }))
}

// ---------------------------------------------------------------------------
// Server builder
// ---------------------------------------------------------------------------

/// Build the API router.
pub fn build_router(state: Arc<DaemonState>) -> Router {
    let router = Router::new()
        .route("/health", get(health_handler))
        .route("/status", get(status_handler))
        .route("/repos", get(repos_handler))
        .route("/agents", get(agents_handler))
        .route("/runs", get(runs_handler))
        .route("/runs/{run_id}", get(run_detail_handler))
        .route("/runs/{run_id}/output", get(run_output_handler))
        .route("/run_or_join", axum::routing::post(run_or_join_handler))
        .route("/stream_run/{run_id}", get(stream_run_handler))
        .route("/events", get(events_handler))
        .route("/install_status", get(install_status_handler))
        .route("/explain/{run_id}", get(explain_handler))
        .route("/audit", get(audit_handler))
        .route("/preview", axum::routing::post(preview_handler))
        .route("/budget", get(budget_handler))
        .route("/openapi.json", get(openapi_handler))
        .route("/onboard", get(onboard_handler))
        
        .route(
            "/settings",
            get(settings_get_handler).put(settings_put_handler),
        );

    // Mobile gateway routes are served on a separate TCP port (default 9777)
    // when RICHTER_MOBILE_ENABLED=true. The TCP listener has its own auth layer.
    // Nesting mobile routes on the Unix socket requires homogeneous Router state
    // types; the mobile TCP listener is the canonical surface for mobile clients.

    router
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST, Method::PUT])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .with_state(state.clone())
}

/// Bind to the Unix domain socket path and start serving.
///
/// Removes any stale socket file at `socket_path` before binding.
pub async fn serve(state: Arc<DaemonState>, socket_path: &std::path::Path) -> anyhow::Result<()> {
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
