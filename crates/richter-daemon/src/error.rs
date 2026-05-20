//! Typed error hierarchy for the Richter daemon API boundary.
//!
//! Internal service methods (run_manager, supervisor, etc.) continue to use
//! `anyhow::Result` for application-level error handling. The `DaemonError` enum
//! is used only at the HTTP API boundary to produce proper HTTP status codes
//! and structured JSON error responses.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::fmt;

/// Typed errors for the daemon API layer.
///
/// Each variant maps to a specific HTTP status code. Internal errors (which wrap
/// `anyhow::Error`) are reported as 500 Internal Server Error, with the detail
/// message hidden from clients in production.
#[derive(Debug)]
pub enum DaemonError {
    /// 401 Unauthorized — authentication failed.
    Auth { reason: String },

    /// 404 Not Found — the requested entity does not exist.
    NotFound { entity: String, id: String },

    /// 400 Bad Request — the request was malformed or invalid.
    BadRequest { reason: String },

    /// 409 Conflict — resource state conflict (e.g., duplicate pairing).
    Conflict { reason: String },

    /// 429 Too Many Requests — rate limit exceeded.
    RateLimited { retry_after_secs: u32 },

    /// 500 Internal Server Error — something unexpected went wrong.
    Internal(anyhow::Error),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth { reason } => write!(f, "authentication failed: {reason}"),
            Self::NotFound { entity, id } => write!(f, "{entity} not found: {id}"),
            Self::BadRequest { reason } => write!(f, "bad request: {reason}"),
            Self::Conflict { reason } => write!(f, "conflict: {reason}"),
            Self::RateLimited { retry_after_secs } => {
                write!(f, "rate limited (retry after {retry_after_secs}s)")
            }
            Self::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Internal(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl IntoResponse for DaemonError {
    fn into_response(self) -> Response {
        let (status, message, retry_after) = match &self {
            Self::Auth { reason } => (StatusCode::UNAUTHORIZED, reason.clone(), None),
            Self::NotFound { entity, id } => (
                StatusCode::NOT_FOUND,
                format!("{entity} not found: {id}"),
                None,
            ),
            Self::BadRequest { reason } => (StatusCode::BAD_REQUEST, reason.clone(), None),
            Self::Conflict { reason } => (StatusCode::CONFLICT, reason.clone(), None),
            Self::RateLimited { retry_after_secs } => (
                StatusCode::TOO_MANY_REQUESTS,
                format!("rate limited, retry after {retry_after_secs}s"),
                Some(*retry_after_secs),
            ),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
                None,
            ),
        };

        let body = Json(serde_json::json!({
            "error": message,
            "status": status.as_u16(),
        }));

        let mut response = (status, body).into_response();

        // Add Retry-After header for rate-limited responses.
        if let Some(secs) = retry_after {
            if let Ok(val) = axum::http::HeaderValue::from_str(&secs.to_string()) {
                response.headers_mut().insert("retry-after", val);
            }
        }

        response
    }
}

impl From<anyhow::Error> for DaemonError {
    fn from(err: anyhow::Error) -> Self {
        Self::Internal(err)
    }
}

/// Convenience type alias for daemon API handler results.
pub type DaemonResult<T> = std::result::Result<T, DaemonError>;
