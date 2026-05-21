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
use serde::Serialize;
use std::fmt;

/// Machine-readable error codes for the daemon API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Authentication failed or token is invalid.
    AuthFailed,
    /// Requested run or resource not found.
    NotFound,
    /// Request body or parameters were malformed.
    BadRequest,
    /// Resource state conflict (e.g. duplicate pairing, approval already decided).
    Conflict,
    /// Rate limit exceeded.
    RateLimited,
    /// Run could not be fingerprinted or deduplicated.
    FingerprintFailed,
    /// Cache read/write error.
    CacheError,
    /// Scheduler could not allocate resources.
    SchedulerUnavailable,
    /// Process spawn or supervision error.
    SpawnFailed,
    /// Invalid command string (injection, forbidden chars, empty).
    InvalidCommand,
    /// Internal unexpected error.
    InternalError,
}

/// Typed errors for the daemon API layer.
///
/// Each variant maps to a specific HTTP status code and carries a machine-readable
/// `ErrorCode`. Clients can switch on `code` for programmatic handling while
/// displaying `message` to users.
#[derive(Debug)]
pub enum DaemonError {
    Auth { reason: String },
    NotFound { entity: String, id: String },
    BadRequest { reason: String },
    Conflict { reason: String },
    RateLimited { retry_after_secs: u32 },
    Fingerprint { reason: String },
    CachePoisoned { reason: String },
    SchedulerUnavailable { reason: String },
    SpawnFailed { reason: String },
    InvalidCommand { reason: String },
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
            Self::Fingerprint { reason } => write!(f, "fingerprint failed: {reason}"),
            Self::CachePoisoned { reason } => write!(f, "cache error: {reason}"),
            Self::SchedulerUnavailable { reason } => write!(f, "scheduler unavailable: {reason}"),
            Self::SpawnFailed { reason } => write!(f, "spawn failed: {reason}"),
            Self::InvalidCommand { reason } => write!(f, "invalid command: {reason}"),
            Self::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl DaemonError {
    /// The machine-readable error code for programmatic error handling.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Auth { .. } => ErrorCode::AuthFailed,
            Self::NotFound { .. } => ErrorCode::NotFound,
            Self::BadRequest { .. } => ErrorCode::BadRequest,
            Self::Conflict { .. } => ErrorCode::Conflict,
            Self::RateLimited { .. } => ErrorCode::RateLimited,
            Self::Fingerprint { .. } => ErrorCode::FingerprintFailed,
            Self::CachePoisoned { .. } => ErrorCode::CacheError,
            Self::SchedulerUnavailable { .. } => ErrorCode::SchedulerUnavailable,
            Self::SpawnFailed { .. } => ErrorCode::SpawnFailed,
            Self::InvalidCommand { .. } => ErrorCode::InvalidCommand,
            Self::Internal(_) => ErrorCode::InternalError,
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
        let code = self.code();
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
            Self::Fingerprint { reason } => (StatusCode::BAD_REQUEST, reason.clone(), None),
            Self::CachePoisoned { reason } => {
                (StatusCode::SERVICE_UNAVAILABLE, reason.clone(), None)
            }
            Self::SchedulerUnavailable { reason } => {
                (StatusCode::SERVICE_UNAVAILABLE, reason.clone(), None)
            }
            Self::SpawnFailed { reason } => {
                (StatusCode::INTERNAL_SERVER_ERROR, reason.clone(), None)
            }
            Self::InvalidCommand { reason } => (StatusCode::BAD_REQUEST, reason.clone(), None),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_string(),
                None,
            ),
        };

        let body = Json(serde_json::json!({
            "code": code,
            "error": message,
            "status": status.as_u16(),
        }));

        let mut response = (status, body).into_response();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_mapping() {
        assert_eq!(
            DaemonError::Auth {
                reason: "bad token".into()
            }
            .code(),
            ErrorCode::AuthFailed
        );
        assert_eq!(
            DaemonError::RateLimited {
                retry_after_secs: 5
            }
            .code(),
            ErrorCode::RateLimited
        );
        assert_eq!(
            DaemonError::Internal(anyhow::anyhow!("boom")).code(),
            ErrorCode::InternalError
        );
    }

    #[test]
    fn test_error_response_shape() {
        let err = DaemonError::BadRequest {
            reason: "empty command".into(),
        };
        let code = err.code();
        assert!(matches!(code, ErrorCode::BadRequest));
    }
}
