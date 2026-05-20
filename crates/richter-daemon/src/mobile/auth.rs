//! Phase 4.2 + 4.4: Device authentication middleware and Ed25519 signature verification.
//!
//! Provides:
//! - `DeviceId` newtype for authenticated device identity
//! - `device_auth_middleware` — Axum middleware that validates device signatures
//! - `required_scope_for_path` — Maps routes to required device scopes
//! - `verify_device_signature` — Ed25519 per-request signature verification
//! - `authenticate_device` — Device existence and revocation check
//! - `body_hash` — SHA-256 body digest helper

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use base64::Engine;
use chrono::Utc;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tracing::warn;

use super::state::MobileGatewayState;

// ---------------------------------------------------------------------------
// DeviceId newtype
// ---------------------------------------------------------------------------

/// Extracted device ID from authentication.
#[derive(Debug, Clone)]
pub struct DeviceId(pub String);

// ---------------------------------------------------------------------------
// Scope mapping
// ---------------------------------------------------------------------------

/// Map request paths to required device scopes (Phase 4.4).
pub fn required_scope_for_path(path: &str) -> Option<&'static str> {
    // Write operations require higher scopes
    if path.contains("/approve") || path.contains("/deny") {
        return Some("approve_actions");
    }
    if path.starts_with("/mobile/v1/runs") {
        return Some("run_commands");
    }
    // Read operations need at least readonly
    if path.starts_with("/mobile/v1/now")
        || path.starts_with("/mobile/v1/status")
        || path.starts_with("/mobile/v1/repos")
        || path.starts_with("/mobile/v1/agents")
        || path.starts_with("/mobile/v1/events")
        || path.starts_with("/mobile/v1/approvals")
    {
        return Some("readonly");
    }
    None
}

// ---------------------------------------------------------------------------
// Body hash helper
// ---------------------------------------------------------------------------

/// Compute SHA-256 hex digest of a byte slice.
pub fn body_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// Device signature verification (Phase 4.2)
// ---------------------------------------------------------------------------

/// Phase 4.2: Verify a device's Ed25519 signature on a request.
/// The signature covers `timestamp + method + path + body_hash`.
pub fn verify_device_signature(
    state: &MobileGatewayState,
    device_id: &str,
    timestamp: &str,
    method: &str,
    path: &str,
    body_hash: &str,
    signature_b64: &str,
) -> bool {
    // Find device's public key
    let pubkey_b64 = {
        let devices = state.devices.read();
        match devices
            .iter()
            .find(|d| d.id == device_id && d.revoked_at.is_none())
        {
            Some(d) => d.device_public_key.clone(),
            None => return false,
        }
    };

    // Decode the public key from base64
    let pubkey_bytes = match base64::engine::general_purpose::STANDARD.decode(&pubkey_b64) {
        Ok(bytes) => bytes,
        Err(_) => {
            warn!("Invalid base64 in device public key for {device_id}");
            return false;
        }
    };

    let pubkey_array: [u8; 32] = match pubkey_bytes.try_into() {
        Ok(arr) => arr,
        Err(_) => {
            warn!("Invalid Ed25519 public key length for device {device_id}");
            return false;
        }
    };
    let verifying_key = match VerifyingKey::from_bytes(&pubkey_array) {
        Ok(vk) => vk,
        Err(_) => {
            warn!("Invalid Ed25519 public key for device {device_id}");
            return false;
        }
    };

    // Decode the signature from base64
    let sig_bytes = match base64::engine::general_purpose::STANDARD.decode(signature_b64) {
        Ok(bytes) => bytes,
        Err(_) => {
            warn!("Invalid base64 in signature for device {device_id}");
            return false;
        }
    };

    let signature = match Signature::from_slice(&sig_bytes) {
        Ok(s) => s,
        Err(_) => {
            warn!("Invalid Ed25519 signature for device {device_id}");
            return false;
        }
    };

    // Construct the message: timestamp + method + path + body_hash
    let message = format!("{timestamp}{method}{path}{body_hash}");

    match verifying_key.verify(message.as_bytes(), &signature) {
        Ok(()) => true,
        Err(_) => {
            warn!("Signature verification failed for device {device_id}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Device authentication (existence + not-revoked)
// ---------------------------------------------------------------------------

/// Authenticate a device by ID. Returns true if the device exists and is not revoked.
/// Also updates the device's last_seen_at timestamp.
pub fn authenticate_device(state: &MobileGatewayState, device_id: &str) -> bool {
    let mut devices = state.devices.write();
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

// ---------------------------------------------------------------------------
// Device auth middleware (Phase 4.2 + 4.3 + 4.4 + 4.5)
// ---------------------------------------------------------------------------

/// Headers expected from the mobile client for device authentication:
/// - `X-Device-ID`: Device identifier (e.g. "mob_a1b2c3d4e5f6")
/// - `X-Timestamp`: Unix epoch seconds (string)
/// - `X-Nonce`: Unique request nonce (UUID)
/// - `X-Signature`: Base64 Ed25519 signature of `timestamp+method+path+body_hash`
/// - `X-Body-Hash`: SHA-256 hex digest of the request body
///
/// Also supports legacy `Authorization: Bearer <pairing_token>` for initial pairing.
pub async fn device_auth_middleware(
    State(state): State<Arc<MobileGatewayState>>,
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    // Pairing endpoints use the legacy bearer token
    if path.starts_with("/mobile/v1/pair") || path == "/mobile/v1/health" {
        // Health and pairing endpoints: allow through (pairing uses its own secret verification)
        return Ok(next.run(req).await);
    }

    // For all other endpoints, require device authentication
    let headers = req.headers();

    // Device signature authentication
    let device_id = match headers.get("X-Device-ID").and_then(|v| v.to_str().ok()) {
        Some(id) => id.to_string(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let timestamp = match headers.get("X-Timestamp").and_then(|v| v.to_str().ok()) {
        Some(ts) => ts.to_string(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let nonce = match headers.get("X-Nonce").and_then(|v| v.to_str().ok()) {
        Some(n) => n.to_string(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let signature = match headers.get("X-Signature").and_then(|v| v.to_str().ok()) {
        Some(sig) => sig.to_string(),
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let body_hash = headers
        .get("X-Body-Hash")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Phase 4.3: Timestamp validation — reject requests older than 60s
    let ts_secs: i64 = match timestamp.parse() {
        Ok(s) => s,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };
    let now_secs = Utc::now().timestamp();
    if (now_secs - ts_secs).unsigned_abs() > 60 {
        warn!(
            "Mobile request from {device_id} rejected: timestamp skew {}s",
            now_secs - ts_secs
        );
        return Err(StatusCode::REQUEST_TIMEOUT);
    }

    // Phase 4.3: Replay protection — nonce must be unique
    if !state.nonce_tracker.check_and_insert(&nonce) {
        warn!("Mobile request from {device_id} rejected: replayed nonce");
        return Err(StatusCode::CONFLICT);
    }

    // Phase 4.2: Verify device signature
    if !verify_device_signature(
        &state,
        &device_id,
        &timestamp,
        method.as_str(),
        &path,
        body_hash,
        &signature,
    ) {
        warn!("Mobile request from {device_id} rejected: invalid signature");
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Phase 4.4: Scope enforcement — check if device has required scope for the endpoint
    let required_scope = required_scope_for_path(&path);
    if let Some(scope) = required_scope {
        if !state.device_has_scope(&device_id, scope) {
            warn!("Mobile request from {device_id} rejected: missing scope '{scope}' for {path}");
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // Phase 4.5: Rate limiting
    if let Some(retry_after) = state.rate_limiter.check(&device_id) {
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            format!("Rate limit exceeded. Retry after {retry_after:.0}s."),
        )
            .into_response();
        response.headers_mut().insert(
            "Retry-After",
            retry_after
                .ceil()
                .to_string()
                .parse()
                .unwrap_or_else(|_| "60".parse().unwrap()),
        );
        return Ok(response);
    }

    // Authentication successful — add device_id to request extensions
    let mut req = req;
    req.extensions_mut().insert(DeviceId(device_id));

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::pairing::MobileDevice;
    use crate::mobile::state::MobileGatewayState;
    use ed25519_dalek::Signer;

    #[test]
    fn test_device_signature_verification() {
        let state = MobileGatewayState::new();

        // Generate a device keypair
        let mut secret_bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut secret_bytes);
        let device_signing_key = ed25519_dalek::SigningKey::from(secret_bytes);
        let device_verifying_key = device_signing_key.verifying_key();
        let pubkey_b64 =
            base64::engine::general_purpose::STANDARD.encode(device_verifying_key.as_bytes());

        // Register device
        let device_id = "mob_testdevice01";
        state.devices.write().push(MobileDevice {
            id: device_id.into(),
            display_name: "Test Device".into(),
            platform: "test".into(),
            device_public_key: pubkey_b64,
            scopes: vec!["readonly".into()],
            created_at: chrono::Utc::now(),
            last_seen_at: chrono::Utc::now(),
            revoked_at: None,
            revocation_reason: None,
            push_enabled: false,
            relay_enabled: false,
            app_version: None,
            os_version: None,
        });

        // Sign a request
        let timestamp = chrono::Utc::now().timestamp().to_string();
        let method = "GET";
        let path = "/mobile/v1/now";
        let body_hash_str = "abc123";
        let message = format!("{timestamp}{method}{path}{body_hash_str}");
        let signature = device_signing_key.sign(message.as_bytes());
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

        // Verify
        assert!(verify_device_signature(
            &state,
            device_id,
            &timestamp,
            method,
            path,
            body_hash_str,
            &sig_b64,
        ));

        // Wrong message should fail
        assert!(!verify_device_signature(
            &state,
            device_id,
            &timestamp,
            "POST",
            path,
            body_hash_str,
            &sig_b64,
        ));
    }

    #[test]
    fn test_scope_mapping() {
        assert_eq!(
            required_scope_for_path("/mobile/v1/approvals/abc/approve"),
            Some("approve_actions")
        );
        assert_eq!(
            required_scope_for_path("/mobile/v1/approvals/abc/deny"),
            Some("approve_actions")
        );
        assert_eq!(required_scope_for_path("/mobile/v1/now"), Some("readonly"));
        assert_eq!(required_scope_for_path("/mobile/v1/health"), None);
        assert_eq!(required_scope_for_path("/mobile/v1/pair"), None);
    }

    #[test]
    fn test_body_hash() {
        let hash = body_hash(b"hello world");
        assert_eq!(hash.len(), 64); // SHA-256 hex = 64 chars
                                    // Deterministic
        assert_eq!(body_hash(b"hello world"), hash);
        // Different input = different hash
        assert_ne!(body_hash(b"hello earth"), hash);
    }
}
