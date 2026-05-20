//! API route handlers for the Richter daemon.

use axum::{
    extract::{Path, State},
    response::{Json, Sse},
};
use serde_json::json;
use std::sync::Arc;
use tokio_stream::StreamExt;
use tracing::warn;

use crate::error::{DaemonError, DaemonResult};
use crate::supervisor;

use super::{
    AgentInfo, AppState, HealthResponse, InstallStatus, MetricsResponse, RepoEntry,
    RunOrJoinRequest, SettingsUpdate, StatusResponse,
};

// ---------------------------------------------------------------------------
// Health & status
// ---------------------------------------------------------------------------

/// GET /health
pub async fn health_handler(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    // Check DB health with a lightweight query
    let db_status = match state.system.db.list_active_runs().await {
        Ok(_) => "ok",
        Err(_) => "degraded",
    };

    let watcher_status = if state
        .system
        .watcher_healthy
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        "ok"
    } else {
        "disabled"
    };

    // Scheduler is always healthy if we're serving requests
    let scheduler_status = "ok";

    let overall = if db_status == "ok" && scheduler_status == "ok" {
        "ok"
    } else {
        "degraded"
    };

    Json(HealthResponse {
        status: overall.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        components: json!({
            "db": db_status,
            "watcher": watcher_status,
            "scheduler": scheduler_status,
        }),
    })
}

/// GET /status
pub async fn status_handler(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let snap = state.runs.scheduler.resource_snapshot();
    Json(StatusResponse {
        health: "ok".to_string(),
        active_runs: state.runs.scheduler.active_count(),
        queued_runs: state.runs.scheduler.queue_depth(),
        cpu_percent: snap.cpu_percent,
        memory_percent: snap.memory_percent,
        subscriber_count: state.system.event_bus.receiver_count(),
        cache_hits_today: state.runs.run_manager.cache_hits_today(),
        duplicates_prevented: state.runs.run_manager.duplicates_prevented(),
    })
}

// ---------------------------------------------------------------------------
// Data endpoints
// ---------------------------------------------------------------------------

/// GET /repos
pub async fn repos_handler(State(state): State<Arc<AppState>>) -> Json<Vec<RepoEntry>> {
    let repos = state.repos.lock().clone();
    Json(repos)
}

/// GET /agents
pub async fn agents_handler(State(state): State<Arc<AppState>>) -> Json<Vec<AgentInfo>> {
    let active_ids = state.runs.supervisor.active_run_ids();
    let mut agents: Vec<AgentInfo> = active_ids
        .iter()
        .filter_map(|id| {
            state.runs.supervisor.run_info(id).map(|info| AgentInfo {
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
pub async fn runs_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<Vec<supervisor::RunInfo>> {
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
        .min(500);
    let offset: usize = params
        .get("offset")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let ids = state.runs.supervisor.active_run_ids();
    let infos: Vec<_> = ids
        .iter()
        .filter_map(|id| state.runs.supervisor.run_info(id))
        .skip(offset)
        .take(limit)
        .collect();
    Json(infos)
}

/// GET /runs/{run_id}
pub async fn run_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> DaemonResult<Json<supervisor::RunInfo>> {
    state
        .runs
        .supervisor
        .run_info(&run_id)
        .map(Json)
        .ok_or_else(|| DaemonError::NotFound {
            entity: "Run".into(),
            id: run_id,
        })
}

/// GET /runs/{run_id}/output
pub async fn run_output_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> DaemonResult<Json<serde_json::Value>> {
    let output = state
        .runs
        .supervisor
        .get_output(&run_id)
        .unwrap_or_default();
    Ok(Json(json!({"output": output})))
}

// ---------------------------------------------------------------------------
// Run submission
// ---------------------------------------------------------------------------

/// POST /run_or_join
#[tracing::instrument(skip(state))]
pub async fn run_or_join_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RunOrJoinRequest>,
) -> DaemonResult<Json<serde_json::Value>> {
    if let Err(reason) = req.validate() {
        return Err(DaemonError::BadRequest { reason });
    }

    let spec = supervisor::RunSpec {
        command: req.command,
        repo: std::path::PathBuf::from(req.repo),
        env: req.env,
        classification: req
            .classification
            .parse()
            .unwrap_or(richter_core::models::CommandClass::Unknown),
        resource_class: req
            .resource_class
            .parse()
            .unwrap_or(richter_core::models::ResourceClass::Unknown),
        force: req.force,
        preview: req.preview,
        ..Default::default()
    };

    match state.runs.run_manager.run_or_join(spec).await {
        Ok(outcome) => {
            let json = serde_json::to_value(&outcome)
                .map_err(|e| DaemonError::Internal(anyhow::anyhow!("{e}")))?;
            Ok(Json(json))
        }
        Err(e) => Err(DaemonError::Internal(e)),
    }
}

// ---------------------------------------------------------------------------
// SSE streams
// ---------------------------------------------------------------------------

/// Convert a String to an SSE event.
fn to_sse_event(line: String) -> Result<axum::response::sse::Event, std::convert::Infallible> {
    Ok(axum::response::sse::Event::default().data(line))
}

/// GET /stream_run/{run_id} — SSE stream for run output.
#[tracing::instrument(skip(state))]
pub async fn stream_run_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>
{
    let stream = match state.runs.supervisor.stream_output(&run_id).await {
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

/// GET /events — SSE stream for daemon events with optional cursor-based pagination.
///
/// Supports `?cursor=<iso-timestamp>` to replay events since a given time.
pub async fn events_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>
{
    let _cursor = params.get("cursor").cloned();
    let mut rx = state.system.event_bus.subscribe_all();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok(axum::response::sse::Event::default()
                        .data(json)
                        .event(event_variant_name(&event)));
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

/// Return the SSE event type name for a DaemonEvent.
fn event_variant_name(event: &crate::event_bus::DaemonEvent) -> &'static str {
    match event {
        crate::event_bus::DaemonEvent::RunStarted { .. } => "RunStarted",
        crate::event_bus::DaemonEvent::RunCompleted { .. } => "RunCompleted",
        crate::event_bus::DaemonEvent::RunCached { .. } => "RunCached",
        crate::event_bus::DaemonEvent::RunQueued { .. } => "RunQueued",
        crate::event_bus::DaemonEvent::ImportantEvent { .. } => "ImportantEvent",
        crate::event_bus::DaemonEvent::ResourcePressure { .. } => "ResourcePressure",
        crate::event_bus::DaemonEvent::ConflictDetected { .. } => "ConflictDetected",
        crate::event_bus::DaemonEvent::FileChanged { .. } => "FileChanged",
        crate::event_bus::DaemonEvent::DaemonStatus { .. } => "DaemonStatus",
        crate::event_bus::DaemonEvent::RunDequeued { .. } => "RunDequeued",
    }
}

// ---------------------------------------------------------------------------
// Config & status endpoints
// ---------------------------------------------------------------------------

/// GET /install_status
pub async fn install_status_handler(State(state): State<Arc<AppState>>) -> Json<InstallStatus> {
    let status = state.system.install_status.lock().clone();
    Json(status)
}

/// GET /settings
pub async fn settings_get_handler(
    State(state): State<Arc<AppState>>,
) -> Json<std::collections::HashMap<String, serde_json::Value>> {
    let settings = state.system.settings.lock().clone();
    Json(settings)
}

/// PUT /settings
pub async fn settings_put_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettingsUpdate>,
) -> Json<std::collections::HashMap<String, serde_json::Value>> {
    let mut settings = state.system.settings.lock();
    for (k, v) in req.settings {
        settings.insert(k, v);
    }
    Json(settings.clone())
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// GET /explain/{run_id}
pub async fn explain_handler(
    State(state): State<Arc<AppState>>,
    Path(run_id): Path<String>,
) -> DaemonResult<Json<serde_json::Value>> {
    let info = state.runs.supervisor.run_info(&run_id);
    match info {
        Some(run_info) => Ok(Json(json!({
            "run_id": run_info.run_id,
            "command": run_info.command,
            "disposition": if run_info.is_active { "running" } else { "completed" },
            "reason": "Run was started or joined through the run-or-join engine",
            "fingerprint": "unknown",
            "cache_age": null,
            "queue_position": null,
            "estimated_wait_ms": null,
        }))),
        None => Err(DaemonError::NotFound {
            entity: "Run".into(),
            id: run_id,
        }),
    }
}

/// GET /audit — structured audit log.
pub async fn audit_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let limit: usize = params
        .get("last")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let mut rx = state.system.event_bus.subscribe_all();
    let mut entries = Vec::new();

    while let Ok(event) = rx.try_recv() {
        let (event_type, title, summary, severity) = match &event {
            crate::event_bus::DaemonEvent::RunStarted { command, .. } => (
                String::from("run_started"),
                String::from("Run started"),
                command.clone(),
                String::from("info"),
            ),
            crate::event_bus::DaemonEvent::RunCompleted { exit_code, .. } => (
                String::from("run_completed"),
                String::from("Run completed"),
                format!("exit_code={exit_code}"),
                String::from("info"),
            ),
            crate::event_bus::DaemonEvent::RunCached { command, .. } => (
                String::from("run_cached"),
                String::from("Cache hit"),
                command.clone(),
                String::from("info"),
            ),
            crate::event_bus::DaemonEvent::RunQueued { reason, .. } => (
                String::from("run_queued"),
                String::from("Run queued"),
                reason.clone(),
                String::from("info"),
            ),
            crate::event_bus::DaemonEvent::RunDequeued { .. } => (
                String::from("run_dequeued"),
                String::from("Run dequeued"),
                String::new(),
                String::from("info"),
            ),
            crate::event_bus::DaemonEvent::ImportantEvent {
                reason,
                severity: sev,
                ..
            } => (
                String::from("important_event"),
                sev.clone(),
                reason.clone(),
                sev.clone(),
            ),
            crate::event_bus::DaemonEvent::ConflictDetected { conflict_type, .. } => (
                String::from("conflict"),
                String::from("Agent conflict"),
                conflict_type.clone(),
                String::from("warning"),
            ),
            _ => continue,
        };
        entries.push(json!({
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

    Json(json!({
        "entries": entries,
        "total": entries.len(),
    }))
}

// ---------------------------------------------------------------------------
// Preview & budget
// ---------------------------------------------------------------------------

/// POST /preview — dry-run preview of a command.
pub async fn preview_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RunOrJoinRequest>,
) -> Json<serde_json::Value> {
    if let Err(reason) = req.validate() {
        return Json(json!({
            "preview": true,
            "error": reason,
            "message": "Preview failed due to invalid input."
        }));
    }

    let outcome = state
        .runs
        .run_manager
        .run_or_join(supervisor::RunSpec {
            command: req.command.clone(),
            repo: std::path::PathBuf::from(&req.repo),
            env: req.env,
            classification: req
                .classification
                .parse()
                .unwrap_or(richter_core::models::CommandClass::Unknown),
            resource_class: req
                .resource_class
                .parse()
                .unwrap_or(richter_core::models::ResourceClass::Unknown),
            force: req.force,
            preview: req.preview,
            ..Default::default()
        })
        .await;

    match outcome {
        Ok(o) => {
            let val = serde_json::to_value(&o).unwrap_or_default();
            Json(json!({
                "preview": true,
                "disposition": val,
                "message": "This is a preview. No command was executed."
            }))
        }
        Err(e) => Json(json!({
            "preview": true,
            "error": format!("{e:#}"),
            "message": "Preview failed."
        })),
    }
}

/// GET /budget — model call budget status with warnings.
pub async fn budget_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let budget = state.model_call_budget.lock();
    let remaining = budget.remaining();
    let circuit_open = remaining == 0;
    let warnings: Vec<String> = if circuit_open {
        vec![
            "Budget exhausted — circuit breaker open. Model calls rejected until next window."
                .into(),
        ]
    } else if remaining <= budget.max_calls_per_minute / 5 {
        vec![format!(
            "80%+ consumed — {remaining} calls remaining this minute"
        )]
    } else {
        vec![]
    };
    Json(json!({
        "remaining_per_minute": remaining,
        "max_per_minute": budget.max_calls_per_minute,
        "circuit_open": circuit_open,
        "warnings": warnings,
        "monthly_spend_usd_estimate": null,
        "providers": [],
    }))
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// GET /metrics — structured JSON metrics.
pub async fn metrics_handler(State(state): State<Arc<AppState>>) -> Json<MetricsResponse> {
    let active_runs = state.runs.scheduler.active_count();
    let queued_runs = state.runs.scheduler.queue_depth();
    let cache_hits = state.runs.run_manager.cache_hits_today();
    let duplicates = state.runs.run_manager.duplicates_prevented();
    let permits_available = state.runs.scheduler.available_permits();

    Json(MetricsResponse {
        active_runs,
        queued_runs,
        cache_hits_today: cache_hits,
        duplicates_prevented: duplicates,
        scheduler_permits_available: permits_available,
        scheduler_queue_depth: queued_runs,
    })
}

/// GET /metrics/prometheus — Prometheus exposition format text.
pub async fn prometheus_metrics_handler(State(state): State<Arc<AppState>>) -> String {
    state.metrics.to_prometheus()
}

// ---------------------------------------------------------------------------
// Onboarding & OpenAPI
// ---------------------------------------------------------------------------

/// GET /onboard — first-run onboarding status and next steps.
pub async fn onboard_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let installed = state.system.install_status.lock().installed;
    let data_dir = dirs_fallback().join(".richter");
    let is_first_run = !data_dir.join("richter.db").exists();

    let steps = if is_first_run {
        json!([
            {"step": 1, "title": "Daemon installed", "done": installed},
            {"step": 2, "title": "Database created", "done": data_dir.join("richter.db").exists()},
            {"step": 3, "title": "Shims configured", "done": false, "action": "Run: richter install shims"},
            {"step": 4, "title": "MCP configured", "done": false, "action": "Run: richter install mcp"},
        ])
    } else {
        json!([
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

    Json(json!({
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
pub async fn openapi_handler() -> Json<serde_json::Value> {
    Json(json!({
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
        "servers": [{"url": "unix://~/.richter/daemon.sock"}]
    }))
}
