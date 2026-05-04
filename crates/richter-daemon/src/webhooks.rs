//! Webhook system for the Richter daemon.
//!
//! Allows users to configure outgoing HTTP webhooks for specific events.
//! Uses tokio for async delivery with retry logic.

use axum::{extract::State, routing::{get, delete}, Json, Router};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A configured webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub enabled: bool,
    pub secret: Option<String>,
    pub created_at: String,
}

/// Webhook state managed by the daemon.
pub struct WebhookState {
    pub webhooks: RwLock<Vec<WebhookConfig>>,
}

impl WebhookState {
    pub fn new() -> Self {
        Self {
            webhooks: RwLock::new(Vec::new()),
        }
    }
}

pub fn webhook_routes() -> Router<Arc<WebhookState>> {
    Router::new()
        .route("/webhooks", get(list_webhooks).post(create_webhook))
        .route("/webhooks/{id}", delete(delete_webhook))
}

async fn list_webhooks(State(state): State<Arc<WebhookState>>) -> Json<Vec<WebhookConfig>> {
    Json(state.webhooks.read().clone())
}

async fn create_webhook(
    State(state): State<Arc<WebhookState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let url = body.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let events: Vec<String> = body
        .get("events")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
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
    };

    state.webhooks.write().push(webhook.clone());
    Json(serde_json::to_value(&webhook).unwrap_or_default())
}

async fn delete_webhook(
    State(state): State<Arc<WebhookState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let mut hooks = state.webhooks.write();
    let before = hooks.len();
    hooks.retain(|h| h.id != id);
    Json(serde_json::json!({
        "deleted": before > hooks.len(),
    }))
}
