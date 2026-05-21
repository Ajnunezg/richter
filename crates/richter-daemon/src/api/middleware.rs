//! HTTP middleware: rate limiting and request ID injection.

use axum::{body::Body, extract::State, http::Request, middleware::Next, response::Response};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::AppState;
use crate::error::{DaemonError, DaemonResult};

/// Default client identifier used when no agent or device header is present.
const DEFAULT_CLIENT_ID: &str = "default";

/// Extract a per-client identifier from the request for rate limiting.
///
/// Checks for:
/// 1. `X-Device-ID` header (mobile devices)
/// 2. `X-Agent-ID` header (MCP agents)
/// 3. Falls back to `DEFAULT_CLIENT_ID` (CLI / local consumers)
fn client_id_from_request(request: &Request<Body>) -> String {
    let headers = request.headers();

    if let Some(device_id) = headers
        .get("X-Device-ID")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return format!("device:{device_id}");
    }

    if let Some(agent_id) = headers
        .get("X-Agent-ID")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        return format!("agent:{agent_id}");
    }

    DEFAULT_CLIENT_ID.to_string()
}

/// Rate-limiting middleware. Health, metrics, onboard, and openapi endpoints are exempt.
/// Uses per-client tracking: mobile devices and MCP agents are rate-limited independently.
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<Body>,
    next: Next,
) -> DaemonResult<Response> {
    state
        .metrics
        .inc(&crate::metrics::MetricCounter::RequestsTotal);

    let path = request.uri().path();

    // Exempt health, metrics, and openapi endpoints from rate limiting
    if path == "/health"
        || path == "/metrics"
        || path == "/metrics/prometheus"
        || path == "/openapi.json"
        || path == "/onboard"
    {
        return Ok(next.run(request).await);
    }

    // Per-client rate limiting based on agent/device identity
    let client_id = client_id_from_request(&request);
    if let Some(retry_after) = state.rate_limiter.check(&client_id) {
        let seconds = retry_after.ceil() as u32;
        state
            .metrics
            .inc(&crate::metrics::MetricCounter::RateLimited);
        return Err(DaemonError::RateLimited {
            retry_after_secs: seconds,
        });
    }

    Ok(next.run(request).await)
}

/// Request ID middleware — attaches a unique `x-request-id` header and logs the request.
pub async fn request_id_middleware(mut req: Request<Body>, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    req.headers_mut().insert(
        axum::http::header::HeaderName::from_static("x-request-id"),
        axum::http::HeaderValue::from_str(&request_id)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("unknown")),
    );
    tracing::info!(request_id = %request_id, method = %req.method(), path = %req.uri().path(), "request started");
    next.run(req).await
}
