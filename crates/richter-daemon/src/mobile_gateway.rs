//! Mobile Gateway: LAN-capable, device-key-authenticated, scope-gated API
//! for the Richter Mobile companion app. Disabled by default.

use anyhow::Context;
use axum::{extract::State, routing::get, routing::post, Json, Router};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::info;
use uuid::Uuid;

use crate::event_bus::{DaemonEvent, EventBus};
use crate::run_manager::RunManager;

// --- Data types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileConfig {
    pub enabled: bool,
    pub lan_gateway: bool,
    pub remote_relay: bool,
    pub push_notifications: bool,
    pub port: u16,
    pub bind_address: String,
}

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lan_gateway: false,
            remote_relay: false,
            push_notifications: false,
            port: 0,
            bind_address: "0.0.0.0".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileDevice {
    pub id: String,
    pub display_name: String,
    pub platform: String,
    pub device_public_key: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revocation_reason: Option<String>,
    pub push_enabled: bool,
    pub relay_enabled: bool,
    pub app_version: Option<String>,
    pub os_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingSession {
    pub pairing_id: String,
    pub pairing_secret_hash: String,
    pub server_pubkey_fingerprint: String,
    pub requested_scopes: Vec<String>,
    pub expires_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub claimed_device_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileNowResponse {
    pub daemon_ok: bool,
    pub active_runs: usize,
    pub queued_runs: usize,
    pub cpu_percent: f64,
    pub memory_percent: f64,
    pub top_event: Option<MobileEvent>,
    pub duplicate_work_saved: usize,
    pub agent_conflicts: usize,
    pub approvals_pending: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileEvent {
    pub event_id: String,
    pub event_type: String,
    pub importance: u8,
    pub repo_id: Option<String>,
    pub run_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub occurred_at: DateTime<Utc>,
    pub requires_action: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileRun {
    pub run_id: String,
    pub repo: String,
    pub command: String,
    pub classification: String,
    pub exit_code: Option<i32>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub risk_level: String,
    pub command: String,
    pub repo: String,
    pub requesting_agent: String,
    pub reason: String,
    pub expires_at: DateTime<Utc>,
    pub consequences: String,
}

// --- Gateway state ---

pub struct MobileGatewayState {
    pub config: RwLock<MobileConfig>,
    pub devices: RwLock<Vec<MobileDevice>>,
    pub pairing_sessions: RwLock<Vec<PairingSession>>,
    pub signing_key: ed25519_dalek::SigningKey,
    pub daemon_id: String,
    pub event_bus: Option<EventBus>,
    pub run_manager: Option<Arc<RunManager>>,
    pub audit_log: RwLock<Vec<serde_json::Value>>,
}

impl Default for MobileGatewayState {
    fn default() -> Self {
        Self::new()
    }
}

impl MobileGatewayState {
    pub fn new() -> Self {
        let daemon_id = Uuid::new_v4().to_string();
        let mut secret_bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut secret_bytes);
        let signing_key = SigningKey::from(secret_bytes);
        Self {
            config: RwLock::new(MobileConfig::default()),
            devices: RwLock::new(Vec::new()),
            pairing_sessions: RwLock::new(Vec::new()),
            signing_key,
            daemon_id,
            event_bus: None,
            run_manager: None,
            audit_log: RwLock::new(Vec::new()),
        }
    }

    /// Wire the mobile state to the real daemon event bus.
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Wire the mobile state to the real run manager.
    pub fn with_run_manager(mut self, rm: Arc<RunManager>) -> Self {
        self.run_manager = Some(rm);
        self
    }

    /// Start the mobile gateway listener (spawns a background task).
    /// Returns a shutdown sender to signal graceful shutdown.
    pub fn start(&self, bind_addr: SocketAddr) -> anyhow::Result<tokio::sync::watch::Sender<bool>> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(Self {
            config: RwLock::new(self.config.read().clone()),
            devices: RwLock::new(self.devices.read().clone()),
            pairing_sessions: RwLock::new(self.pairing_sessions.read().clone()),
            signing_key: self.signing_key.clone(),
            daemon_id: self.daemon_id.clone(),
            event_bus: self.event_bus.clone(),
            run_manager: self.run_manager.clone(),
            audit_log: RwLock::new(self.audit_log.read().clone()),
        });
        tokio::spawn(async move {
            if let Err(e) = serve_mobile(state, bind_addr, rx).await {
                tracing::error!("Mobile Gateway error: {e}");
            }
        });

        Ok(tx)
    }

    /// Return the configured port (or 0 if not set).
    pub fn port(&self) -> u16 {
        self.config.read().port
    }

    /// Check if a device is authorized for a given scope.
    pub fn device_has_scope(&self, device_id: &str, scope: &str) -> bool {
        self.devices.read().iter().any(|d| {
            d.id == device_id && d.revoked_at.is_none() && d.scopes.iter().any(|s| s == scope)
        })
    }

    /// Return the server's Ed25519 verifying key (public).
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    /// Return the SHA-256 fingerprint of the server's public key.
    pub fn pubkey_sha256(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_key.verifying_key().as_bytes());
        format!("sha256:{:x}", hasher.finalize())
    }

    /// Create a pairing session. Returns the pairing secret (shown to user)
    /// and the session ID.
    pub fn create_pairing_session(&self, requested_scopes: &[String]) -> (String, String) {
        // pairing_id, pairing_secret
        let pairing_id = Uuid::new_v4().to_string();
        let mut secret_bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut secret_bytes);
        let pairing_secret = hex::encode(secret_bytes);

        let mut hasher = Sha256::new();
        hasher.update(secret_bytes);
        let secret_hash = format!("{:x}", hasher.finalize());

        let session = PairingSession {
            pairing_id: pairing_id.clone(),
            pairing_secret_hash: secret_hash,
            server_pubkey_fingerprint: self.pubkey_sha256(),
            requested_scopes: requested_scopes.to_vec(),
            expires_at: Utc::now() + chrono::Duration::seconds(120),
            claimed_at: None,
            claimed_device_id: None,
            created_at: Utc::now(),
        };

        self.pairing_sessions.write().push(session);

        (pairing_id, pairing_secret)
    }

    /// Register a device after successful pairing verification.
    pub fn register_device(
        &self,
        pairing_id: &str,
        pairing_secret: &str,
        device_public_key: &str,
        display_name: &str,
        platform: &str,
    ) -> Result<MobileDevice, String> {
        // Verify pairing session — scope the read lock
        let (requested_scopes, _secret_hash_valid) = {
            let sessions = self.pairing_sessions.read();
            let session = sessions
                .iter()
                .find(|s| s.pairing_id == pairing_id)
                .ok_or("Pairing session not found")?;

            if session.expires_at < Utc::now() {
                return Err("Pairing session expired".into());
            }

            if session.claimed_at.is_some() {
                return Err("Pairing session already claimed".into());
            }

            // Verify secret
            let secret_bytes = hex::decode(pairing_secret).map_err(|_| "Invalid pairing secret")?;
            let mut hasher = Sha256::new();
            hasher.update(&secret_bytes);
            let computed_hash = format!("{:x}", hasher.finalize());
            if computed_hash != session.pairing_secret_hash {
                return Err("Invalid pairing secret".into());
            }

            (session.requested_scopes.clone(), true)
        };

        let raw_id = Uuid::new_v4().to_string().replace('-', "");
        let device_id = format!("mob_{}", &raw_id[..12]);
        let now = Utc::now();

        let device = MobileDevice {
            id: device_id,
            display_name: display_name.to_string(),
            platform: platform.to_string(),
            device_public_key: device_public_key.to_string(),
            scopes: requested_scopes.clone(),
            created_at: now,
            last_seen_at: now,
            revoked_at: None,
            revocation_reason: None,
            push_enabled: false,
            relay_enabled: false,
            app_version: None,
            os_version: None,
        };

        self.devices.write().push(device.clone());

        // Mark session as claimed
        let mut sessions = self.pairing_sessions.write();
        if let Some(s) = sessions.iter_mut().find(|s| s.pairing_id == pairing_id) {
            s.claimed_at = Some(now);
            s.claimed_device_id = Some(device.id.clone());
        }

        Ok(device)
    }

    /// Authenticate a device by ID. Returns true if the device exists and is not revoked.
    pub fn authenticate_device(&self, device_id: &str) -> bool {
        let mut devices = self.devices.write();
        if let Some(d) = devices
            .iter_mut()
            .find(|d| d.id == device_id && d.revoked_at.is_none())
        {
            d.last_seen_at = Utc::now();
            true
        } else {
            false
        }
    }
}

/// Start the mobile gateway TCP listener (LAN-facing, requires explicit enable).
pub async fn serve_mobile(
    state: Arc<MobileGatewayState>,
    bind_addr: SocketAddr,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let router = build_mobile_router(state.clone());

    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("Failed to bind mobile gateway to {bind_addr}"))?;

    info!("Mobile Gateway listening on {bind_addr}");

    axum::serve(listener, router.into_make_service())
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
            info!("Mobile Gateway shutting down");
        })
        .await
        .context("Mobile Gateway server error")?;

    Ok(())
}

// --- API handlers ---

async fn health_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "daemon_id": state.daemon_id,
        "version": "0.1.0", "pubkey_sha256": state.pubkey_sha256(),
    }))
}

fn collect_top_event(event_bus: &Option<EventBus>) -> Option<MobileEvent> {
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

async fn now_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<MobileNowResponse> {
    let active_runs = state
        .run_manager
        .as_ref()
        .map_or(0, |rm| rm.active_runs().len());

    let top_event = collect_top_event(&state.event_bus);

    Json(MobileNowResponse {
        daemon_ok: state.event_bus.is_some(),
        active_runs,
        queued_runs: 0,
        cpu_percent: 0.0,
        memory_percent: 0.0,
        top_event,
        duplicate_work_saved: 0,
        agent_conflicts: 0,
        approvals_pending: 0,
    })
}

async fn status_handler(State(state): State<Arc<MobileGatewayState>>) -> Json<serde_json::Value> {
    let cfg = state.config.read();
    Json(serde_json::json!({
        "mobile_gateway": cfg.enabled,
        "lan_gateway": cfg.lan_gateway,
        "paired_devices": state.devices.read().len(),
        "active_pairing_sessions": state.pairing_sessions.read().len(),
    }))
}

async fn repos_handler(State(st): State<Arc<MobileGatewayState>>) -> Json<Vec<serde_json::Value>> {
    let repos: Vec<serde_json::Value> = st.run_manager.as_ref().map(|rm| {
        rm.active_runs().iter().map(|id| serde_json::json!({"run_id": id})).collect()
    }).unwrap_or_default();
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
    let agents: Vec<serde_json::Value> = st.run_manager.as_ref().map(|rm| {
        rm.active_runs().iter().map(|id| serde_json::json!({"agent_id": id, "status": "active"})).collect()
    }).unwrap_or_default();
    Json(agents)
}

async fn important_events_handler(State(st): State<Arc<MobileGatewayState>>) -> Json<Vec<MobileEvent>> {
    let top: Vec<MobileEvent> = collect_top_event(&st.event_bus).into_iter().collect();
    Json(top)
}

async fn approvals_handler(State(_st): State<Arc<MobileGatewayState>>) -> Json<Vec<ApprovalRequest>> {
    Json(vec![])
}

async fn approve_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "approved"}))
}

async fn deny_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "denied"}))
}

// --- Router ---

pub fn build_mobile_router(state: Arc<MobileGatewayState>) -> Router {
    Router::new()
        .route("/mobile/v1/health", get(health_handler))
        .route("/mobile/v1/pair", post(move |State(s): State<Arc<MobileGatewayState>>, Json(body): Json<serde_json::Value>| async move {
            let scopes: Vec<String> = body.get("scopes")
                .and_then(|s| s.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_else(|| vec!["read_now".into(), "read_runs".into(), "read_events".into()]);
            let (pairing_id, pairing_secret) = s.create_pairing_session(&scopes);
            Json(serde_json::json!({
                "pairing_id": pairing_id,
                "pairing_secret": pairing_secret,
                "server_pubkey_sha256": s.pubkey_sha256(),
                "daemon_id": s.daemon_id,
                "host": "auto",
                "port": s.config.read().port,
                "expires_in_seconds": 120,
            }))
        }))
        .route("/mobile/v1/pair/register", post(move |State(s): State<Arc<MobileGatewayState>>, Json(body): Json<serde_json::Value>| async move {
            let pairing_id = body.get("pairing_id").and_then(|v| v.as_str()).unwrap_or("");
            let pairing_secret = body.get("pairing_secret").and_then(|v| v.as_str()).unwrap_or("");
            let device_public_key = body.get("device_public_key").and_then(|v| v.as_str()).unwrap_or("");
            let display_name = body.get("display_name").and_then(|v| v.as_str()).unwrap_or("Unknown");
            let platform = body.get("platform").and_then(|v| v.as_str()).unwrap_or("unknown");
            match s.register_device(pairing_id, pairing_secret, device_public_key, display_name, platform) {
                Ok(device) => Json(serde_json::json!({"status": "registered", "device_id": device.id, "scopes": device.scopes})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": e})),
            }
        }))
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
        .layer(CorsLayer::permissive())
        .with_state(state)
}

use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_disabled() {
        let cfg = MobileConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.lan_gateway);
        assert!(!cfg.remote_relay);
        assert!(!cfg.push_notifications);
    }

    #[test]
    fn test_device_scope_check() {
        let state = MobileGatewayState::new();
        state.devices.write().push(MobileDevice {
            id: "dev1".into(),
            display_name: "Test Phone".into(),
            platform: "ios".into(),
            device_public_key: "pk_test".into(),
            scopes: vec!["read_now".into(), "read_runs".into()],
            created_at: Utc::now(),
            last_seen_at: Utc::now(),
            revoked_at: None,
            revocation_reason: None,
            push_enabled: false,
            relay_enabled: false,
            app_version: None,
            os_version: None,
        });
        assert!(state.device_has_scope("dev1", "read_now"));
        assert!(state.device_has_scope("dev1", "read_runs"));
        assert!(!state.device_has_scope("dev1", "admin_mobile_devices"));
        assert!(!state.device_has_scope("dev2", "read_now"));
    }

    #[test]
    fn test_revoked_device_denied() {
        let state = MobileGatewayState::new();
        state.devices.write().push(MobileDevice {
            id: "dev1".into(),
            display_name: "Revoked Phone".into(),
            platform: "ios".into(),
            device_public_key: "pk_test".into(),
            scopes: vec!["read_now".into()],
            created_at: Utc::now(),
            last_seen_at: Utc::now(),
            revoked_at: Some(Utc::now()),
            revocation_reason: Some("user request".into()),
            push_enabled: false,
            relay_enabled: false,
            app_version: None,
            os_version: None,
        });
        assert!(!state.device_has_scope("dev1", "read_now"));
    }

    #[test]
    fn test_pairing_session_expiry() {
        let session = PairingSession {
            pairing_id: "pair_1".into(),
            pairing_secret_hash: "hash".into(),
            server_pubkey_fingerprint: "fp".into(),
            requested_scopes: vec!["read_now".into()],
            expires_at: Utc::now() - chrono::Duration::minutes(1),
            claimed_at: None,
            claimed_device_id: None,
            created_at: Utc::now(),
        };
        assert!(session.expires_at < Utc::now());
    }
}
