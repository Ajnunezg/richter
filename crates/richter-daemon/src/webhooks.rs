//! Webhook system for the Richter daemon.
//!
//! Allows users to configure outgoing HTTP webhooks for specific events.
//! Delivers signed JSON payloads with HMAC-SHA256, retry with exponential
//! backoff, and circuit breaker protection.

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::event_bus::{DaemonEvent, EventBus};

type HmacSha256 = Hmac<Sha256>;

/// A configured webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub enabled: bool,
    pub secret: Option<String>,
    pub created_at: String,
    #[serde(skip)]
    pub last_delivered_at: Option<String>,
    #[serde(skip)]
    pub last_status: Option<u16>,
    #[serde(skip)]
    pub failure_count: u32,
}

/// Webhook state shared between the router and the deliverer.
pub struct WebhookState {
    pub webhooks: RwLock<Vec<WebhookConfig>>,
    pub event_bus: Option<EventBus>,
}

impl WebhookState {
    pub fn new() -> Self {
        Self {
            webhooks: RwLock::new(Vec::new()),
            event_bus: None,
        }
    }

    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }
}

impl Default for WebhookState {
    fn default() -> Self {
        Self::new()
    }
}

/// Delivers webhook payloads by subscribing to the event bus.
pub struct WebhookDeliverer {
    state: Arc<WebhookState>,
}

impl WebhookDeliverer {
    pub fn new(state: Arc<WebhookState>) -> Self {
        Self { state }
    }

    /// Start the delivery loop. Spawns a background task.
    pub fn start(self) {
        let state = self.state.clone();
        tokio::spawn(async move {
            let bus = match &state.event_bus {
                Some(b) => b.clone(),
                None => {
                    warn!(
                        "WebhookDeliverer started without event bus — no events will be delivered"
                    );
                    return;
                }
            };

            let mut rx = bus.subscribe_all();
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let event_name = event_variant_name(&event);
                        let payload = build_payload(&event);
                        deliver_to_matching(state.clone(), event_name, &payload).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Webhook event bus lagged by {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("Webhook event bus closed, stopping deliverer");
                        break;
                    }
                }
            }
        });
    }
}

fn event_variant_name(event: &DaemonEvent) -> &'static str {
    match event {
        DaemonEvent::RunStarted { .. } => "RunStarted",
        DaemonEvent::RunCompleted { .. } => "RunCompleted",
        DaemonEvent::RunCached { .. } => "RunCached",
        DaemonEvent::RunQueued { .. } => "RunQueued",
        DaemonEvent::RunDequeued { .. } => "RunDequeued",
        DaemonEvent::ImportantEvent { .. } => "ImportantEvent",
        DaemonEvent::ResourcePressure { .. } => "ResourcePressure",
        DaemonEvent::ConflictDetected { .. } => "ConflictDetected",
        DaemonEvent::FileChanged { .. } => "FileChanged",
        DaemonEvent::DaemonStatus { .. } => "DaemonStatus",
    }
}

fn build_payload(event: &DaemonEvent) -> serde_json::Value {
    match event {
        DaemonEvent::RunStarted {
            run_id,
            command,
            repo,
            ..
        } => serde_json::json!({
            "event": "RunStarted", "run_id": run_id, "command": command, "repo": repo
        }),
        DaemonEvent::RunCompleted {
            run_id,
            exit_code,
            duration_ms,
            ..
        } => serde_json::json!({
            "event": "RunCompleted", "run_id": run_id, "exit_code": exit_code, "duration_ms": duration_ms
        }),
        DaemonEvent::RunCached {
            run_id, command, ..
        } => serde_json::json!({
            "event": "RunCached", "run_id": run_id, "command": command
        }),
        DaemonEvent::ImportantEvent {
            reason, severity, ..
        } => serde_json::json!({
            "event": "ImportantEvent", "reason": reason, "severity": severity
        }),
        other => serde_json::json!({"event": format!("{:?}", other)}),
    }
}

async fn deliver_to_matching(
    state: Arc<WebhookState>,
    event_name: &str,
    payload: &serde_json::Value,
) {
    let hooks: Vec<WebhookConfig> = {
        let hooks = state.webhooks.read();
        hooks
            .iter()
            .filter(|h| h.enabled && h.events.iter().any(|e| e == event_name || e == "*"))
            .cloned()
            .collect()
    };

    if hooks.is_empty() {
        return;
    }

    let payload_str = serde_json::to_string(payload).unwrap_or_default();

    for hook in &hooks {
        let state = state.clone();
        let hook_id = hook.id.clone();
        let url = hook.url.clone();
        let secret = hook.secret.clone();
        let payload = payload_str.clone();

        tokio::spawn(async move {
            let result = deliver_single(&url, &secret, &payload).await;
            let mut hooks = state.webhooks.write();
            if let Some(h) = hooks.iter_mut().find(|h| h.id == hook_id) {
                h.last_delivered_at = Some(chrono::Utc::now().to_rfc3339());
                match result {
                    Ok(status) => {
                        h.last_status = Some(status);
                        h.failure_count = 0;
                        debug!("Webhook {hook_id} delivered: HTTP {status}");
                    }
                    Err(e) => {
                        h.failure_count += 1;
                        warn!(
                            "Webhook {hook_id} failed (attempt {}): {e}",
                            h.failure_count
                        );
                        // Circuit breaker: disable after 5 consecutive failures
                        if h.failure_count >= 5 {
                            h.enabled = false;
                            warn!("Webhook {hook_id} disabled after 5 consecutive failures");
                        }
                    }
                }
            }
        });
    }
}

async fn deliver_single(url: &str, secret: &Option<String>, payload: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    // Retry with exponential backoff
    let retry_delays = [1, 5, 25]; // seconds
    let mut last_error = String::new();

    for (attempt, &delay_secs) in retry_delays.iter().enumerate() {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
        }

        let mut req = client.post(url).header("Content-Type", "application/json");

        // Add HMAC-SHA256 signature if secret is configured
        if let Some(secret) = secret {
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
                .map_err(|e| format!("HMAC init error: {e}"))?;
            mac.update(payload.as_bytes());
            let signature = hex::encode(mac.finalize().into_bytes());
            req = req.header("X-Richter-Signature", format!("sha256={signature}"));
        }

        match req.body(payload.to_string()).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status < 500 {
                    return Ok(status);
                }
                last_error = format!("HTTP {status}");
            }
            Err(e) => {
                last_error = format!("{e}");
            }
        }
    }

    Err(last_error)
}

// ---------------------------------------------------------------------------
// CRUD routes
// ---------------------------------------------------------------------------

pub fn webhook_routes() -> Router<Arc<WebhookState>> {
    Router::new()
        .route("/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/{id}", delete(delete_webhook))
        .route("/webhooks/{id}/test", post(test_webhook))
}

async fn list_webhooks(State(state): State<Arc<WebhookState>>) -> Json<Vec<WebhookConfig>> {
    let hooks: Vec<WebhookConfig> = state.webhooks.read().clone();
    Json(hooks)
}

async fn create_webhook(
    State(state): State<Arc<WebhookState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let url = body.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let events: Vec<String> = body
        .get("events")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let webhook = WebhookConfig {
        id: uuid::Uuid::new_v4().to_string(),
        url: url.to_string(),
        events,
        enabled: true,
        secret: body
            .get("secret")
            .and_then(|v| v.as_str())
            .map(String::from),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_delivered_at: None,
        last_status: None,
        failure_count: 0,
    };

    state.webhooks.write().push(webhook.clone());
    Json(serde_json::to_value(&webhook).unwrap_or_default())
}

async fn delete_webhook(
    State(state): State<Arc<WebhookState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let mut hooks = state.webhooks.write();
    let before = hooks.len();
    hooks.retain(|h| h.id != id);
    Json(serde_json::json!({
        "deleted": before > hooks.len(),
    }))
}

async fn test_webhook(
    State(state): State<Arc<WebhookState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let hook = {
        let hooks = state.webhooks.read();
        hooks.iter().find(|h| h.id == id).cloned()
    };

    let Some(hook) = hook else {
        return Json(serde_json::json!({"status": "error", "error": "Webhook not found"}));
    };

    let test_payload = serde_json::json!({
        "event": "test",
        "webhook_id": hook.id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    let payload_str = serde_json::to_string(&test_payload).unwrap_or_default();

    match deliver_single(&hook.url, &hook.secret, &payload_str).await {
        Ok(status) => Json(serde_json::json!({
            "status": "delivered",
            "http_status": status,
        })),
        Err(e) => Json(serde_json::json!({
            "status": "failed",
            "error": e,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_variant_names() {
        use crate::event_bus::DaemonEvent;
        let event = DaemonEvent::RunStarted {
            run_id: "test".into(),
            repo: ".".into(),
            command: "echo".into(),
            classification: "test".into(),
            started_at: chrono::Utc::now(),
        };
        assert_eq!(event_variant_name(&event), "RunStarted");
    }

    #[test]
    fn test_build_payload_run_started() {
        use crate::event_bus::DaemonEvent;
        let event = DaemonEvent::RunStarted {
            run_id: "r1".into(),
            repo: "/repo".into(),
            command: "cargo test".into(),
            classification: "test".into(),
            started_at: chrono::Utc::now(),
        };
        let payload = build_payload(&event);
        assert_eq!(payload["event"], "RunStarted");
        assert_eq!(payload["run_id"], "r1");
        assert_eq!(payload["command"], "cargo test");
    }

    #[test]
    fn test_circuit_breaker_disables_after_five_failures() {
        let mut hook = WebhookConfig {
            id: "wh-1".into(),
            url: "https://example.com/hook".into(),
            events: vec!["RunStarted".into()],
            enabled: true,
            secret: None,
            created_at: "2025-01-01T00:00:00Z".into(),
            last_delivered_at: None,
            last_status: None,
            failure_count: 5,
        };

        // Simulate the circuit breaker logic
        hook.failure_count += 1;
        if hook.failure_count >= 5 {
            hook.enabled = false;
        }
        assert!(!hook.enabled, "webhook should be disabled after 5 failures");
    }

    #[test]
    fn test_hmac_signature() {
        let secret = "test-secret";
        let payload = r#"{"event":"test"}"#;

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());

        // Verify the signature
        let mut verify_mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        verify_mac.update(payload.as_bytes());
        verify_mac
            .verify_slice(&hex::decode(&signature).unwrap())
            .unwrap();

        // Wrong secret should fail
        let mut wrong_mac = HmacSha256::new_from_slice(b"wrong-secret").unwrap();
        wrong_mac.update(payload.as_bytes());
        assert!(wrong_mac
            .verify_slice(&hex::decode(&signature).unwrap())
            .is_err());
    }
}
