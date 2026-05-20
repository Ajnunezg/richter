//! Phase 4.4 + 4.6: Device pairing and registration.
//!
//! Contains pairing session management, device registration,
//! and the pairing-related route handlers.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use super::state::MobileGatewayState;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Pairing methods on MobileGatewayState
// ---------------------------------------------------------------------------

impl MobileGatewayState {
    /// Create a pairing session. Returns the pairing secret (shown to user)
    /// and the session ID.
    pub fn create_pairing_session(&self, requested_scopes: &[String]) -> (String, String) {
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
    /// Default scope is `readonly` (Phase 4.4).
    pub fn register_device(
        &self,
        pairing_id: &str,
        pairing_secret: &str,
        device_public_key: &str,
        display_name: &str,
        platform: &str,
    ) -> Result<MobileDevice, String> {
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

            let secret_bytes = hex::decode(pairing_secret).map_err(|_| "Invalid pairing secret")?;
            let mut hasher = Sha256::new();
            hasher.update(&secret_bytes);
            let computed_hash = format!("{:x}", hasher.finalize());
            let is_valid: bool = subtle::ConstantTimeEq::ct_eq(
                computed_hash.as_bytes(),
                session.pairing_secret_hash.as_bytes(),
            )
            .into();
            if !is_valid {
                return Err("Invalid pairing secret".into());
            }

            (session.requested_scopes.clone(), true)
        };

        // Phase 4.4: Default new devices to `readonly` if no scopes requested
        let scopes = if requested_scopes.is_empty() {
            vec!["readonly".to_string()]
        } else {
            requested_scopes
        };

        let raw_id = Uuid::new_v4().to_string().replace('-', "");
        let device_id = format!("mob_{}", &raw_id[..12]);
        let now = Utc::now();

        let device = MobileDevice {
            id: device_id,
            display_name: display_name.to_string(),
            platform: platform.to_string(),
            device_public_key: device_public_key.to_string(),
            scopes: scopes.clone(),
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
}

// ---------------------------------------------------------------------------
// Pairing route handlers
// ---------------------------------------------------------------------------

pub async fn pairing_handler(
    State(s): State<Arc<MobileGatewayState>>,
    Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let scopes: Vec<String> = body
        .get("scopes")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_else(|| vec!["readonly".into(), "read_runs".into(), "read_events".into()]);
    let (pairing_id, pairing_secret) = s.create_pairing_session(&scopes);
    axum::Json(serde_json::json!({
        "pairing_id": pairing_id,
        "pairing_secret": pairing_secret,
        "server_pubkey_sha256": s.pubkey_sha256(),
        "daemon_id": s.daemon_id,
        "host": "auto",
        "port": s.config.read().port,
        "expires_in_seconds": 120,
    }))
}

pub async fn pair_register_handler(
    State(s): State<Arc<MobileGatewayState>>,
    Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let pairing_id = body
        .get("pairing_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let pairing_secret = body
        .get("pairing_secret")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let device_public_key = body
        .get("device_public_key")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");
    let platform = body
        .get("platform")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    match s.register_device(
        pairing_id,
        pairing_secret,
        device_public_key,
        display_name,
        platform,
    ) {
        Ok(device) => {
            // Persist device to SQLite (Phase 4.6)
            let device_clone = device.clone();
            let db_state = s.clone();
            tokio::spawn(async move {
                db_state.persist_device(&device_clone).await;
            });
            axum::Json(serde_json::json!({
                "status": "registered",
                "device_id": device.id,
                "scopes": device.scopes
            }))
        }
        Err(e) => axum::Json(serde_json::json!({"status": "error", "error": e})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::state::MobileGatewayState;

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

    #[test]
    fn test_default_readonly_scope() {
        let state = MobileGatewayState::new();
        // Create pairing session with empty scopes
        let (pairing_id, pairing_secret) = state.create_pairing_session(&[]);

        // Override the requested_scopes to empty (simulate a client sending no scopes)
        {
            let mut sessions = state.pairing_sessions.write();
            if let Some(s) = sessions.iter_mut().find(|s| s.pairing_id == pairing_id) {
                s.requested_scopes = vec![];
            }
        }

        let result = state.register_device(
            &pairing_id,
            &pairing_secret,
            "dGVzdA==",
            "Test Phone",
            "ios",
        );

        let device = result.expect("registration should succeed");
        assert_eq!(device.scopes, vec!["readonly"]);
    }
}
