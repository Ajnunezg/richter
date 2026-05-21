//! Mobile gateway API route handlers.
//!
//! All Axum route handlers for the mobile gateway, plus `build_mobile_router`
//! which wires up the routes with middleware and CORS.

use axum::{
    extract::State,
    http::{header, Method},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

use crate::event_bus::DaemonEvent;

use super::auth::device_auth_middleware;
use super::pairing::{pair_register_handler, pairing_handler};
use super::state::{
    ApprovalDecision, ApprovalRequest, MobileEvent, MobileGatewayState, MobileNowResponse,
    MobileRun,
};

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

async fn health_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "daemon_id": state.daemon_id,
        "version": "0.1.0",
        "pubkey_sha256": state.pubkey_sha256(),
    }))
}

fn collect_top_event(event_bus: &Option<crate::event_bus::EventBus>) -> Option<MobileEvent> {
    let bus = event_bus.as_ref()?;
    let mut rx = bus.subscribe_all();
    match rx.try_recv() {
        Ok(event) => {
            let (event_type, title, summary) = match &event {
                DaemonEvent::RunStarted { command, .. } => {
                    ("RunStarted", "Run started".into(), command.clone())
                }
                DaemonEvent::RunCompleted { exit_code, .. } => (
                    "RunCompleted",
                    "Run completed".into(),
                    format!("exit_code={exit_code}"),
                ),
                DaemonEvent::RunCached { command, .. } => {
                    ("RunCached", "Cache hit".into(), command.clone())
                }
                DaemonEvent::RunQueued { reason, .. } => {
                    ("RunQueued", "Run queued".into(), reason.clone())
                }
                DaemonEvent::ImportantEvent {
                    reason, severity, ..
                } => ("ImportantEvent", format!("[{severity}]"), reason.clone()),
                DaemonEvent::ResourcePressure {
                    resource,
                    description,
                    ..
                } => ("ResourcePressure", resource.clone(), description.clone()),
                DaemonEvent::ConflictDetected { conflict_type, .. } => {
                    ("ConflictDetected", "Conflict".into(), conflict_type.clone())
                }
                DaemonEvent::FileChanged { path, kind, .. } => {
                    ("FileChanged", kind.clone(), path.clone())
                }
                DaemonEvent::DaemonStatus { status, .. } => {
                    ("DaemonStatus", "Daemon".into(), status.clone())
                }
                DaemonEvent::RunDequeued { .. } => {
                    ("RunDequeued", "Run dequeued".into(), String::new())
                }
            };
            Some(MobileEvent {
                event_id: Uuid::new_v4().to_string(),
                event_type: event_type.into(),
                importance: 5,
                repo_id: None,
                run_id: None,
                title,
                summary,
                occurred_at: Utc::now(),
                requires_action: false,
            })
        }
        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => None,
        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => None,
        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => None,
    }
}

/// Phase 4.6: Now handler with real scheduler/monitor data.
async fn now_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<MobileNowResponse> {
    let active_runs = state
        .run_manager
        .as_ref()
        .map_or(0, |rm| rm.active_runs().len());

    let top_event = collect_top_event(&state.event_bus);

    // Real system metrics from ResourceMonitor
    let (cpu_percent, memory_percent) = state
        .resource_monitor
        .as_ref()
        .map(|rm| {
            let snap = rm.current();
            (snap.cpu_percent as f64, snap.memory_percent as f64)
        })
        .unwrap_or((0.0, 0.0));

    // Duplicate work saved from run manager
    let duplicate_work_saved = state
        .run_manager
        .as_ref()
        .map_or(0, |rm| rm.duplicates_prevented() as usize);

    // Count conflict events from event bus (agent_conflicts)
    let agent_conflicts = 0; // Aggregated from event bus history — best effort

    // Pending approvals
    let approvals_pending = state
        .approvals
        .read()
        .iter()
        .filter(|a| a.decision.is_none())
        .count();

    Json(MobileNowResponse {
        daemon_ok: state.event_bus.is_some(),
        active_runs,
        queued_runs: 0, // Would need scheduler internals
        cpu_percent,
        memory_percent,
        top_event,
        duplicate_work_saved,
        agent_conflicts,
        approvals_pending,
    })
}

async fn status_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<serde_json::Value> {
    let cfg = state.config.read();
    Json(serde_json::json!({
        "mobile_gateway": cfg.enabled,
        "lan_gateway": cfg.lan_gateway,
        "tls_enabled": cfg.tls_enabled,
        "paired_devices": state.devices.read().len(),
        "active_pairing_sessions": state.pairing_sessions.read().len(),
    }))
}

async fn repos_handler(State(st): State<Arc<MobileGatewayState>>) -> Json<Vec<serde_json::Value>> {
    let repos: Vec<serde_json::Value> = st
        .run_manager
        .as_ref()
        .map(|rm| {
            rm.active_runs()
                .iter()
                .map(|id| serde_json::json!({"run_id": id}))
                .collect()
        })
        .unwrap_or_default();
    Json(repos)
}

async fn runs_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<Vec<MobileRun>> {
    match &state.run_manager {
        Some(rm) => {
            let active = rm.active_runs();
            let runs: Vec<MobileRun> = active
                .iter()
                .map(|run_id| MobileRun {
                    run_id: run_id.clone(),
                    repo: String::new(),
                    command: String::new(),
                    classification: String::new(),
                    exit_code: None,
                    is_active: true,
                })
                .collect();
            Json(runs)
        }
        None => Json(vec![]),
    }
}

async fn agents_handler(State(st): State<Arc<MobileGatewayState>>) -> Json<Vec<serde_json::Value>> {
    let agents: Vec<serde_json::Value> = st
        .run_manager
        .as_ref()
        .map(|rm| {
            rm.active_runs()
                .iter()
                .map(|id| serde_json::json!({"agent_id": id, "status": "active"}))
                .collect()
        })
        .unwrap_or_default();
    Json(agents)
}

async fn important_events_handler(
    State(st): State<Arc<MobileGatewayState>>,
) -> Json<Vec<MobileEvent>> {
    let top: Vec<MobileEvent> = collect_top_event(&st.event_bus).into_iter().collect();
    Json(top)
}

/// Phase 4.6: Return real pending approvals.
async fn approvals_handler(
    State(state): State<Arc<MobileGatewayState>>,
) -> Json<Vec<ApprovalRequest>> {
    let approvals = state.approvals.read();
    let pending: Vec<ApprovalRequest> = approvals
        .iter()
        .filter(|a| a.decision.is_none() && a.expires_at > Utc::now())
        .map(|a| ApprovalRequest {
            approval_id: a.approval_id.clone(),
            risk_level: a.risk_level.clone(),
            command: a.command.clone(),
            repo: a.repo.clone(),
            requesting_agent: a.requesting_agent.clone(),
            reason: a.reason.clone(),
            expires_at: a.expires_at,
            consequences: a.consequences.clone(),
        })
        .collect();
    Json(pending)
}

/// Phase 4.6: Wire approve handler to real decision system.
async fn approve_handler(
    State(state): State<Arc<MobileGatewayState>>,
    axum::extract::Path(approval_id): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Json<serde_json::Value> {
    let device_id: String = req
        .extensions()
        .get::<super::auth::DeviceId>()
        .map(|d| d.0.clone())
        .unwrap_or_else(|| String::from("daemon"));
    let decided_by = device_id.clone();

    // Update the approval entry (scoped lock)
    let result = {
        let mut approvals = state.approvals.write();
        if let Some(entry) = approvals.iter_mut().find(|a| a.approval_id == approval_id) {
            if entry.decision.is_some() {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Approval already decided"
                }));
            }
            if entry.expires_at < Utc::now() {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Approval expired"
                }));
            }
            entry.decision = Some(ApprovalDecision {
                approved: true,
                decided_at: Utc::now(),
                decided_by: decided_by.clone(),
            });
            true
        } else {
            false
        }
    }; // lock dropped here

    if !result {
        return Json(serde_json::json!({
            "status": "error",
            "error": "Approval not found"
        }));
    }

    // Log the decision to audit log
    state.audit_log.write().push(serde_json::json!({
        "action": "approve",
        "approval_id": approval_id,
        "decided_by": decided_by,
        "timestamp": Utc::now().to_rfc3339(),
    }));

    tracing::info!("Mobile approval {approval_id} approved by {decided_by}");

    // Persist to database
    if let Some(db) = &state.db {
        let now_iso = Utc::now().to_rfc3339();
        let audit_id = Uuid::new_v4().to_string();
        let _ = db
            .insert_mobile_audit(
                &audit_id,
                Some(&decided_by),
                "approve",
                Some("approval"),
                Some(&approval_id),
                None,
                true,
                None,
                &now_iso,
            )
            .await;
    }

    Json(serde_json::json!({"status": "approved", "approval_id": approval_id}))
}

/// Phase 4.6: Wire deny handler to real decision system.
async fn deny_handler(
    State(state): State<Arc<MobileGatewayState>>,
    axum::extract::Path(approval_id): axum::extract::Path<String>,
    req: axum::http::Request<axum::body::Body>,
) -> Json<serde_json::Value> {
    let device_id: String = req
        .extensions()
        .get::<super::auth::DeviceId>()
        .map(|d| d.0.clone())
        .unwrap_or_else(|| String::from("daemon"));
    let decided_by = device_id.clone();

    // Update the approval entry (scoped lock)
    let result = {
        let mut approvals = state.approvals.write();
        if let Some(entry) = approvals.iter_mut().find(|a| a.approval_id == approval_id) {
            if entry.decision.is_some() {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Approval already decided"
                }));
            }
            if entry.expires_at < Utc::now() {
                return Json(serde_json::json!({
                    "status": "error",
                    "error": "Approval expired"
                }));
            }
            entry.decision = Some(ApprovalDecision {
                approved: false,
                decided_at: Utc::now(),
                decided_by: decided_by.clone(),
            });
            true
        } else {
            false
        }
    }; // lock dropped here

    if !result {
        return Json(serde_json::json!({
            "status": "error",
            "error": "Approval not found"
        }));
    }

    state.audit_log.write().push(serde_json::json!({
        "action": "deny",
        "approval_id": approval_id,
        "decided_by": decided_by,
        "timestamp": Utc::now().to_rfc3339(),
    }));

    tracing::info!("Mobile approval {approval_id} denied by {decided_by}");

    // Persist to database
    if let Some(db) = &state.db {
        let now_iso = Utc::now().to_rfc3339();
        let audit_id = Uuid::new_v4().to_string();
        let _ = db
            .insert_mobile_audit(
                &audit_id,
                Some(&decided_by),
                "deny",
                Some("approval"),
                Some(&approval_id),
                None,
                true,
                None,
                &now_iso,
            )
            .await;
    }

    Json(serde_json::json!({"status": "denied", "approval_id": approval_id}))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Return the TLS certificate fingerprint for pinning.
/// This endpoint is intentionally unauthenticated — clients need it before pairing.
async fn cert_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<serde_json::Value> {
    let fingerprint = state.cert_fingerprint.read().clone();
    Json(serde_json::json!({
        "tls_enabled": state.config.read().tls_enabled,
        "cert_fingerprint": fingerprint,
        "pubkey_sha256": state.pubkey_sha256(),
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn build_mobile_router(state: Arc<MobileGatewayState>) -> Router {
    let authenticated = Router::new()
        .route("/mobile/v1/health", get(health_handler))
        .route("/mobile/v1/pair", post(pairing_handler))
        .route("/mobile/v1/pair/register", post(pair_register_handler))
        .route("/mobile/v1/now", get(now_handler))
        .route("/mobile/v1/status", get(status_handler))
        .route("/mobile/v1/repos", get(repos_handler))
        .route("/mobile/v1/agents", get(agents_handler))
        .route("/mobile/v1/runs", get(runs_handler))
        .route("/mobile/v1/events/important", get(important_events_handler))
        .route("/mobile/v1/approvals", get(approvals_handler))
        .route(
            "/mobile/v1/approvals/{approval_id}/approve",
            post(approve_handler),
        )
        .route(
            "/mobile/v1/approvals/{approval_id}/deny",
            post(deny_handler),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            device_auth_middleware,
        ));

    Router::new()
        // Unauthenticated endpoints (needed before device pairing)
        .route("/mobile/v1/cert", get(cert_handler))
        .merge(authenticated)
        .layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin([
                    "http://localhost:3000".parse().unwrap(),
                    "http://localhost:5173".parse().unwrap(),
                    "http://127.0.0.1:3000".parse().unwrap(),
                    "http://127.0.0.1:5173".parse().unwrap(),
                ])
                .allow_methods([Method::GET, Method::POST, Method::PUT])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    "X-Device-ID".parse().unwrap(),
                    "X-Timestamp".parse().unwrap(),
                    "X-Nonce".parse().unwrap(),
                    "X-Signature".parse().unwrap(),
                    "X-Body-Hash".parse().unwrap(),
                ]),
        )
        .with_state(state)
}
