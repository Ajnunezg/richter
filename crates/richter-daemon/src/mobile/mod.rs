//! Mobile Gateway: LAN-capable, device-key-authenticated, scope-gated API
//! for the Richter Mobile companion app. Disabled by default.
//!
//! # Security layers (Phase 4)
//! - **4.1 TLS**: Self-signed cert on first startup, `localhost:9777` only by default
//! - **4.2 Device signing**: Ed25519 per-request signature verification
//! - **4.3 Replay protection**: Timestamp window (60s) + nonce bloom filter
//! - **4.4 Scope enforcement**: Per-device scopes wired into auth middleware
//! - **4.5 Rate limiting**: Token bucket per device (60 req/min)
//! - **4.6 Real data**: Wired to scheduler, monitor, and decision system

pub mod auth;
pub mod nonce;
pub mod pairing;
pub mod rate_limit;
pub mod routes;
pub mod state;
pub mod tls;

use anyhow::Context;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

// Re-export all public types so that `crate::mobile::MobileGatewayState` etc. work.
pub use auth::{
    authenticate_device, body_hash, device_auth_middleware, required_scope_for_path,
    verify_device_signature, DeviceId,
};
pub use nonce::{NonceTracker, NONCE_CAPACITY};
pub use pairing::{MobileDevice, PairingSession};
pub use rate_limit::RateLimiter;
pub use routes::build_mobile_router;
pub use state::{
    ApprovalDecision, ApprovalEntry, ApprovalRequest, MobileConfig, MobileEvent,
    MobileGatewayState, MobileNowResponse, MobileRun,
};
pub use tls::MobileTlsConfig;

// ---------------------------------------------------------------------------
// TLS-aware mobile gateway server
// ---------------------------------------------------------------------------

/// Start the mobile gateway TCP listener (LAN-facing, requires explicit enable).
/// When TLS is enabled, wraps each connection with rustls TLS 1.3.
/// When TLS is disabled, serves plain HTTP (suitable for reverse-proxy setups).
pub async fn serve_mobile(
    state: Arc<MobileGatewayState>,
    bind_addr: SocketAddr,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    use_tls: bool,
    data_dir: &Path,
) -> anyhow::Result<()> {
    let router = build_mobile_router(state.clone());

    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("Failed to bind mobile gateway to {bind_addr}"))?;

    if use_tls {
        // Phase 4.1: Set up TLS with self-signed cert on first startup.
        let tls_config =
            tls::setup_tls(data_dir).with_context(|| "Failed to set up TLS for mobile gateway")?;

        // Store fingerprint on state so /cert endpoint can serve it.
        *state.cert_fingerprint.write() = Some(tls_config.cert_fingerprint.clone());

        info!(
            "Mobile Gateway listening on https://{bind_addr} (TLS 1.3, ECDSA P-256, fingerprint={})",
            tls_config.cert_fingerprint
        );

        // Accept TCP connections and wrap each with TLS, then serve Axum.
        loop {
            tokio::select! {
                accept_result = listener.accept() => {
                    let (stream, remote_addr) = accept_result.with_context(|| "Failed to accept TCP connection")?;
                    let tls_acceptor = tls_config.acceptor.clone();
                    let router = router.clone();

                    tokio::spawn(async move {
                        match tls_acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                // Convert the tls stream into a hyper service.
                                let svc = router.into_service();
                                let svc = hyper_util::service::TowerToHyperService::new(svc);
                                let io = hyper_util::rt::TokioIo::new(tls_stream);
                                let _ = hyper::server::conn::http1::Builder::new()
                                    .serve_connection(io, svc)
                                    .await;
                            }
                            Err(e) => {
                                tracing::warn!("Mobile TLS accept failed from {remote_addr}: {e}");
                            }
                        }
                    });
                }
                _ = shutdown.changed() => {
                    info!("Mobile Gateway shutting down");
                    break;
                }
            }
        }
    } else {
        // Plain HTTP mode — suitable for reverse proxy (nginx, caddy) termination.
        // Clear the fingerprint since there's no TLS certificate.
        *state.cert_fingerprint.write() = None;

        info!("Mobile Gateway listening on http://{bind_addr} (TLS disabled)");

        axum::serve(listener, router.into_make_service())
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
                info!("Mobile Gateway shutting down");
            })
            .await
            .context("Mobile gateway server error")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::pairing::MobileDevice;
    use crate::mobile::state::MobileConfig;
    use chrono::Utc;

    #[test]
    fn test_default_config_disabled() {
        let cfg = MobileConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.lan_gateway);
        assert!(!cfg.remote_relay);
        assert!(!cfg.push_notifications);
        assert!(cfg.tls_enabled);
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
}
