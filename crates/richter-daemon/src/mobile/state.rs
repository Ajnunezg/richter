//! Mobile gateway state and shared data types.
//!
//! Contains `MobileGatewayState`, `MobileConfig`, and the response/event structs
//! used by the mobile gateway API endpoints.

use chrono::{DateTime, Utc};
use ed25519_dalek::{SigningKey, VerifyingKey};
use parking_lot::RwLock;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::event_bus::EventBus;
use crate::run_manager::RunManager;
use crate::scheduler::ResourceMonitor;

use super::nonce::NonceTracker;
use super::pairing::MobileDevice;
use super::rate_limit::RateLimiter;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobileConfig {
    pub enabled: bool,
    pub lan_gateway: bool,
    pub remote_relay: bool,
    pub push_notifications: bool,
    pub port: u16,
    pub bind_address: String,
    /// Whether to use TLS (default: true).
    pub tls_enabled: bool,
}

impl Default for MobileConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            lan_gateway: false,
            remote_relay: false,
            push_notifications: false,
            port: 0,
            bind_address: "127.0.0.1".into(),
            tls_enabled: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Response / event types
// ---------------------------------------------------------------------------

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

/// In-memory approval request tracking.
#[derive(Debug, Clone)]
pub struct ApprovalEntry {
    pub approval_id: String,
    pub risk_level: String,
    pub command: String,
    pub repo: String,
    pub requesting_agent: String,
    pub reason: String,
    pub consequences: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decision: Option<ApprovalDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecision {
    pub approved: bool,
    pub decided_at: DateTime<Utc>,
    pub decided_by: String,
}

// ---------------------------------------------------------------------------
// Gateway state
// ---------------------------------------------------------------------------

pub struct MobileGatewayState {
    pub config: RwLock<MobileConfig>,
    pub devices: RwLock<Vec<MobileDevice>>,
    pub pairing_sessions: RwLock<Vec<super::pairing::PairingSession>>,
    pub signing_key: SigningKey,
    pub daemon_id: String,
    pub pairing_token: String,
    pub event_bus: Option<EventBus>,
    pub run_manager: Option<Arc<RunManager>>,
    pub resource_monitor: Option<Arc<ResourceMonitor>>,
    pub audit_log: RwLock<Vec<serde_json::Value>>,
    pub db: Option<Arc<richter_core::db::Database>>,
    /// Phase 4.3: Nonce tracker for replay protection.
    pub nonce_tracker: NonceTracker,
    /// Phase 4.5: Per-device rate limiter.
    pub rate_limiter: RateLimiter,
    /// Phase 4.6: Pending approval requests.
    pub approvals: RwLock<Vec<ApprovalEntry>>,
    /// Phase 4.1: TLS certificate fingerprint (populated after TLS setup).
    pub cert_fingerprint: RwLock<Option<String>>,
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

        let mut pbytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut OsRng, &mut pbytes);
        let pairing_token = hex::encode(pbytes);

        Self {
            config: RwLock::new(MobileConfig::default()),
            devices: RwLock::new(Vec::new()),
            pairing_sessions: RwLock::new(Vec::new()),
            signing_key,
            daemon_id,
            pairing_token,
            event_bus: None,
            run_manager: None,
            resource_monitor: None,
            audit_log: RwLock::new(Vec::new()),
            db: None,
            nonce_tracker: NonceTracker::new(),
            rate_limiter: RateLimiter::default(),
            approvals: RwLock::new(Vec::new()),
            cert_fingerprint: RwLock::new(None),
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

    /// Wire the mobile state to the resource monitor.
    pub fn with_resource_monitor(mut self, monitor: Arc<ResourceMonitor>) -> Self {
        self.resource_monitor = Some(monitor);
        self
    }

    /// Wire the mobile state to the SQLite database for device persistence.
    pub fn with_db(mut self, db: Arc<richter_core::db::Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// Load persisted devices from SQLite into memory (Phase 4.6).
    pub async fn load_devices_from_db(&self) -> anyhow::Result<()> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        let rows = db.list_mobile_devices().await?;
        let mut devices = self.devices.write();
        devices.clear();
        for row in rows {
            let push_enabled = row.push_enabled();
            let relay_enabled = row.relay_enabled();
            let scopes: Vec<String> =
                serde_json::from_str(&row.scopes_json).unwrap_or_else(|_| vec!["readonly".into()]);
            devices.push(MobileDevice {
                id: row.id,
                display_name: row.display_name,
                platform: row.platform,
                device_public_key: row.device_public_key,
                scopes,
                created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
                last_seen_at: row.last_seen_at.parse().unwrap_or_else(|_| Utc::now()),
                revoked_at: row.revoked_at.and_then(|s| s.parse().ok()),
                revocation_reason: row.revocation_reason,
                push_enabled,
                relay_enabled,
                app_version: row.app_version,
                os_version: row.os_version,
            });
        }
        tracing::info!("Loaded {} device(s) from SQLite", devices.len());
        Ok(())
    }

    /// Persist a device to SQLite (Phase 4.6).
    pub(super) async fn persist_device(&self, device: &MobileDevice) {
        if let Some(db) = &self.db {
            let scopes_json = serde_json::to_string(&device.scopes).unwrap_or_else(|_| "[]".into());
            let created_at = device.created_at.to_rfc3339();
            let last_seen_at = device.last_seen_at.to_rfc3339();
            if let Err(e) = db
                .upsert_mobile_device(
                    &device.id,
                    &device.display_name,
                    &device.platform,
                    &device.device_public_key,
                    &scopes_json,
                    &created_at,
                    &last_seen_at,
                )
                .await
            {
                tracing::warn!("Failed to persist mobile device to SQLite: {e}");
            }
        }
    }

    /// Start the mobile gateway listener (spawns a background task).
    /// Returns a shutdown sender to signal graceful shutdown.
    pub fn start(
        &self,
        bind_addr: std::net::SocketAddr,
        data_dir: &std::path::Path,
    ) -> anyhow::Result<tokio::sync::watch::Sender<bool>> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(Self {
            config: RwLock::new(self.config.read().clone()),
            devices: RwLock::new(self.devices.read().clone()),
            pairing_sessions: RwLock::new(self.pairing_sessions.read().clone()),
            signing_key: self.signing_key.clone(),
            daemon_id: self.daemon_id.clone(),
            pairing_token: self.pairing_token.clone(),
            event_bus: self.event_bus.clone(),
            run_manager: self.run_manager.clone(),
            resource_monitor: self.resource_monitor.clone(),
            audit_log: RwLock::new(self.audit_log.read().clone()),
            db: self.db.clone(),
            nonce_tracker: NonceTracker::new(),
            rate_limiter: RateLimiter::default(),
            approvals: RwLock::new(self.approvals.read().clone()),
            cert_fingerprint: RwLock::new(None),
        });

        // Phase 4.1: TLS setup
        let use_tls = self.config.read().tls_enabled;
        let data_dir = data_dir.to_path_buf();

        tokio::spawn(async move {
            if let Err(e) = super::serve_mobile(state, bind_addr, rx, use_tls, &data_dir).await {
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
}
