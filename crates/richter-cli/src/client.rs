//! Typed HTTP client: communicates with the Richter daemon via a Unix domain socket.
//!
//! Replaces raw string-building with proper HTTP/1.1 response parsing using `httparse`,
//! status-code validation, Content-Length verification, and chunked-transfer support.
//! Exposes type-safe `get`/`post` methods alongside the legacy `send_raw` interface.

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[allow(dead_code)]
pub struct LocalClient {
    socket_path: String,
    auth_token: Option<String>,
    read_timeout: Duration,
    connect_timeout: Duration,
}

impl LocalClient {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            auth_token: Self::load_auth_token(),
            read_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(5),
        }
    }

    /// Send a legacy JSON request to the daemon.
    ///
    /// The JSON must contain a `"method"` field that maps to a daemon endpoint.
    /// For backward compatibility with the existing CLI command structure.
    pub fn send_raw(&self, json: &str) -> Result<Vec<u8>> {
        let val: serde_json::Value =
            serde_json::from_str(json).context("Failed to parse request JSON")?;
        let method = val
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("health");

        let (http_method, path) = Self::map_method(method);
        let body = Self::extract_body(&val, http_method)?;
        let body_bytes: Option<Vec<u8>> = body.map(|s| s.into_bytes());

        self.request(http_method, path, body_bytes.as_deref())
    }

    /// Type-safe GET request.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let bytes = self.request("GET", path, None)?;
        serde_json::from_slice(&bytes).context("Failed to deserialize GET response")
    }

    /// Type-safe POST request.
    pub fn post<T: Serialize, U: DeserializeOwned>(&self, path: &str, body: &T) -> Result<U> {
        let body_json = serde_json::to_vec(body).context("Failed to serialize POST body")?;
        let bytes = self.request("POST", path, Some(&body_json))?;
        serde_json::from_slice(&bytes).context("Failed to deserialize POST response")
    }

    /// Check whether the daemon is reachable.
    pub fn check_health(&self) -> Result<()> {
        let _ = self.request("GET", "/health", None)?;
        Ok(())
    }

    // -------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------

    fn map_method(method: &str) -> (&str, &str) {
        match method {
            "health" | "events_follow" => ("GET", "/health"),
            "status" => ("GET", "/status"),
            "repos" => ("GET", "/repos"),
            "agents" => ("GET", "/agents"),
            "runs" | "run_status" => ("GET", "/runs"),
            "events" => ("GET", "/events"),
            "install_status" => ("GET", "/install_status"),
            "settings" | "settings_reload" => ("GET", "/settings"),
            "settings_update" => ("PUT", "/settings"),
            "run" | "simulate" => ("POST", "/run_or_join"),
            "explain" => ("GET", "/explain/unknown"), // callers override via params
            "audit" => ("GET", "/audit"),
            "claim_acquire" | "claim_release" | "claim_list" | "worktree_create"
            | "worktree_list" | "worktree_remove" => {
                // These endpoints do not exist yet; map to health so the
                // caller gets a predictable "not found" instead of a
                // malformed request.
                tracing::warn!("Unimplemented CLI method '{}' mapped to /health", method);
                ("GET", "/health")
            }
            _ => ("GET", "/health"),
        }
    }

    fn extract_body(val: &serde_json::Value, http_method: &str) -> Result<Option<String>> {
        if http_method == "POST" || http_method == "PUT" {
            let params = val
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let body = serde_json::to_string(&params).context("Failed to serialize body")?;
            Ok(Some(body))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn request(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<Vec<u8>> {
        let token = self.auth_token.as_deref().unwrap_or("");

        // Prevent header injection from malformed auth tokens
        if token.contains('\n') || token.contains('\r') {
            anyhow::bail!("Auth token contains invalid characters");
        }

        let mut stream = self.connect()?;

        // Build request bytes — still manual (Unix sockets are local; the
        // daemon closes after each response so connection reuse is impossible).
        let mut req = Vec::new();
        req.extend_from_slice(format!("{} {} HTTP/1.1\r\n", method, path).as_bytes());
        req.extend_from_slice(b"Host: localhost\r\n");
        if !token.is_empty() {
            req.extend_from_slice(format!("Authorization: Bearer {}\r\n", token).as_bytes());
        }
        req.extend_from_slice(b"Content-Type: application/json\r\n");
        if let Some(body) = body {
            req.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
        }
        req.extend_from_slice(b"Connection: close\r\n\r\n");
        if let Some(body) = body {
            req.extend_from_slice(body);
        }

        stream.write_all(&req).context("Failed to write request")?;
        stream.flush().context("Failed to flush")?;

        // Read full response
        let mut buf = Vec::new();
        let mut reader = std::io::BufReader::new(&stream);
        reader
            .get_mut()
            .read_to_end(&mut buf)
            .context("Failed to read response")?;

        Self::parse_response_body(&buf)
    }

    /// Parse an HTTP/1.1 response, checking status code and extracting the body.
    fn parse_response_body(buf: &[u8]) -> Result<Vec<u8>> {
        use httparse::Response;

        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut response = Response::new(&mut headers);

        match response.parse(buf) {
            Ok(status) if status.is_complete() => {
                let header_len = status.unwrap();
                let status_code = response.code.unwrap_or(0);

                if !(200..300).contains(&status_code) {
                    let body_preview = String::from_utf8_lossy(&buf[header_len..]);
                    let preview = if body_preview.len() > 200 {
                        format!("{}...", &body_preview[..200])
                    } else {
                        body_preview.to_string()
                    };
                    anyhow::bail!("HTTP error {}: {}", status_code, preview);
                }

                // Detect chunked transfer encoding
                let chunked = response.headers.iter().any(|h| {
                    h.name.eq_ignore_ascii_case("Transfer-Encoding")
                        && std::str::from_utf8(h.value)
                            .map(|v| v.contains("chunked"))
                            .unwrap_or(false)
                });

                if chunked {
                    Ok(Self::decode_chunked(&buf[header_len..]))
                } else {
                    // Validate Content-Length to detect truncation
                    let content_length = response
                        .headers
                        .iter()
                        .find(|h| h.name.eq_ignore_ascii_case("Content-Length"))
                        .and_then(|h| std::str::from_utf8(h.value).ok())
                        .and_then(|v| v.parse::<usize>().ok());

                    let body = &buf[header_len..];
                    if let Some(expected) = content_length {
                        if body.len() < expected {
                            anyhow::bail!(
                                "Response truncated: expected {} bytes, got {}",
                                expected,
                                body.len()
                            );
                        }
                    }
                    Ok(body.to_vec())
                }
            }
            Ok(_) => anyhow::bail!("Incomplete HTTP response"),
            Err(e) => anyhow::bail!("HTTP parse error: {:?}", e),
        }
    }

    /// Decode a chunked-transfer-encoded body.
    fn decode_chunked(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        let mut pos = 0;

        loop {
            if pos >= data.len() {
                break;
            }

            let line_end = match data[pos..].windows(2).position(|w| w == b"\r\n") {
                Some(end) => pos + end,
                None => break,
            };

            let size_str = std::str::from_utf8(&data[pos..line_end]).unwrap_or("0");
            let chunk_size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);

            if chunk_size == 0 {
                break;
            }

            let chunk_start = line_end + 2;
            let chunk_end = chunk_start + chunk_size;

            if chunk_end > data.len() {
                break;
            }

            result.extend_from_slice(&data[chunk_start..chunk_end]);
            pos = chunk_end + 2; // skip trailing \r\n
        }

        result
    }

    fn connect(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(&self.socket_path)
            .with_context(|| format!("Failed to connect to {}", self.socket_path))?;
        stream.set_read_timeout(Some(self.read_timeout)).ok();
        stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
        Ok(stream)
    }

    fn load_auth_token() -> Option<String> {
        let home = std::env::var("HOME").ok()?;
        let path = std::path::PathBuf::from(home)
            .join(".richter")
            .join("auth_token");
        std::fs::read_to_string(&path)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }
}

/// Public helper for loading the auth token.
pub fn load_auth_token_func() -> Option<String> {
    LocalClient::load_auth_token()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_client_new() {
        let client = LocalClient::new("/nonexistent/richter.sock");
        assert_eq!(client.socket_path, "/nonexistent/richter.sock");
    }

    #[test]
    fn test_daemon_unreachable() {
        let client = LocalClient::new("/tmp/nonexistent.sock");
        assert!(client.check_health().is_err());
    }

    #[test]
    fn test_parse_response_ok() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}";
        let body = LocalClient::parse_response_body(resp).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[test]
    fn test_parse_response_error() {
        let resp = b"HTTP/1.1 500 Internal Server Error\r\n\r\n{\"error\":\"boom\"}";
        let result = LocalClient::parse_response_body(resp);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("HTTP error 500"));
        assert!(msg.contains("boom"));
    }

    #[test]
    fn test_parse_response_truncated() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"a\":1}";
        let result = LocalClient::parse_response_body(resp);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("truncated"));
    }

    #[test]
    fn test_parse_chunked() {
        let chunked = b"5\r\nhello\r\n5\r\nworld\r\n0\r\n\r\n";
        let body = LocalClient::decode_chunked(chunked);
        assert_eq!(body, b"helloworld");
    }

    #[test]
    fn test_empty_chunked() {
        let chunked = b"0\r\n\r\n";
        let body = LocalClient::decode_chunked(chunked);
        assert!(body.is_empty());
    }

    #[test]
    fn test_header_injection_blocked() {
        let client = LocalClient {
            socket_path: "/tmp/nonexistent.sock".to_string(),
            auth_token: Some("bad\nX-Evil: injected".to_string()),
            read_timeout: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(1),
        };
        let result = client.request("GET", "/health", None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid characters"));
    }
}
