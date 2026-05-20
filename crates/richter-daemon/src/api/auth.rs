//! Auth middleware, token generation, and scope enforcement.

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, Method, Request},
    middleware::Next,
    response::Response,
};
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

use crate::api::AppState;
use crate::error::{DaemonError, DaemonResult};

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

/// Validates the `Authorization: Bearer <token>[:<scope>]` header against the stored token.
///
/// Supports scope-based authorization:
/// - Token format: `hex-token` (backward compatible, gets write scope)
/// - Token format: `hex-token:scope` (read, write, or admin)
///
/// Scope enforcement:
/// - `read`: GET endpoints only
/// - `write`: all endpoints
/// - `admin`: all endpoints (reserved for future)
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> DaemonResult<Response> {
    let expected = state.auth.auth_token.get().ok_or_else(|| {
        tracing::error!("Auth token not yet initialized");
        DaemonError::Internal(anyhow::anyhow!("Auth token not initialized"))
    })?;

    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(token_with_scope) = auth_header.strip_prefix("Bearer ") {
        // Parse optional scope suffix: "hex-token" or "hex-token:scope"
        let (token, scope) = if let Some((t, s)) = token_with_scope.split_once(':') {
            (t, s.to_string())
        } else {
            // Backward compatible: no scope suffix -> write access
            (token_with_scope, "write".to_string())
        };

        use subtle::ConstantTimeEq;
        if token.as_bytes().ct_eq(expected.as_bytes()).into() {
            // Update the stored scope
            *state.auth.auth_scope.lock() = scope.clone();

            // Enforce scope-based authorization
            let method = request.method().clone();
            let path = request.uri().path().to_string();

            if !is_scope_allowed(&scope, &method, &path) {
                state
                    .metrics
                    .inc(&crate::metrics::MetricCounter::AuthFailures);
                return Err(DaemonError::Auth {
                    reason: format!("Insufficient scope '{scope}' for {method} {path}"),
                });
            }

            return Ok(next.run(request).await);
        }
    }

    state
        .metrics
        .inc(&crate::metrics::MetricCounter::AuthFailures);
    Err(DaemonError::Auth {
        reason: "Invalid or missing auth token".into(),
    })
}

/// Check whether a given scope is allowed to perform an HTTP method on a path.
pub fn is_scope_allowed(scope: &str, method: &Method, path: &str) -> bool {
    // Admin scope can do anything
    if scope == "admin" {
        return true;
    }

    // Write scope can do anything
    if scope == "write" {
        return true;
    }

    // Read scope: only GET, HEAD, OPTIONS
    if scope == "read" {
        if method == Method::GET || method == Method::HEAD || method == Method::OPTIONS {
            return true;
        }
        // Allow POST to /preview (it's a read-only operation)
        if method == Method::POST && path == "/preview" {
            return true;
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Token generation and verification
// ---------------------------------------------------------------------------

/// Generate a random auth token and write it to a file with 0600 permissions.
pub fn generate_auth_token(path: &Path) -> anyhow::Result<String> {
    let bytes = rand_bytes();
    let token = hex::encode(bytes);

    // Write token to file with restrictive permissions
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, &token)?;

    // Set file permissions to 0600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(path)?.permissions();
        let mut new_perms = perms;
        new_perms.set_mode(0o600);
        std::fs::set_permissions(path, new_perms)?;
    }

    Ok(token)
}

/// Verify that a sensitive file has restrictive permissions (0600 on Unix).
/// Warns and corrects if permissions are too loose.
pub fn verify_restrictive_permissions(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::metadata(path)?.permissions();
        let mode = perms.mode() & 0o777;
        if mode != 0o600 {
            warn!("Correcting loose permissions on {:?}: {:o}", path, mode);
            let mut new_perms = perms;
            new_perms.set_mode(0o600);
            std::fs::set_permissions(path, new_perms)?;
        }
    }

    Ok(())
}

/// Generate cryptographically random bytes for token generation.
fn rand_bytes() -> [u8; 32] {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scope_allowed() {
        let read = Method::GET;
        let write = Method::POST;
        assert!(is_scope_allowed("admin", &read, "/"));
        assert!(is_scope_allowed("admin", &write, "/"));
        assert!(is_scope_allowed("write", &read, "/"));
        assert!(is_scope_allowed("write", &write, "/"));
        assert!(is_scope_allowed("read", &read, "/"));
        assert!(!is_scope_allowed("read", &write, "/"));
        assert!(is_scope_allowed("read", &Method::POST, "/preview"));
    }
}
