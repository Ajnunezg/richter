//! Daemon API client for the MCP server.
//!
//! Connects to the Richter daemon over its Unix domain socket and makes
//! real HTTP calls for all MCP tool handlers. Falls back to offline
//! responses when the daemon is unreachable.

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// A low-level client for communicating with the Richter daemon
/// over its Unix domain socket.
pub struct DaemonApiClient {
    socket_path: String,
    auth_token: Option<String>,
    read_timeout: Duration,
}

impl DaemonApiClient {
    /// Create a new client targeting the default socket path.
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            auth_token: load_auth_token(),
            read_timeout: Duration::from_secs(10),
        }
    }

    /// Send an HTTP request to the daemon and return the JSON response body.
    fn send_request(&self, method: &str, path: &str, body: Option<JsonValue>) -> Result<JsonValue> {
        #[cfg(not(unix))]
        {
            anyhow::bail!("Unix socket transport is not available on this platform");
        }

        #[cfg(unix)]
        {
            let mut stream = UnixStream::connect(&self.socket_path)
                .with_context(|| format!("Failed to connect to daemon at {}", self.socket_path))?;
            stream.set_read_timeout(Some(self.read_timeout)).ok();

            let token = self.auth_token.as_deref().unwrap_or("");

            let body_str = match &body {
                Some(b) => serde_json::to_string(b).unwrap_or_default(),
                None => String::new(),
            };

            let use_body = method == "POST" && !body_str.is_empty();
            let content_length = if use_body { body_str.len() } else { 0 };

            let mut req = String::new();
            req.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
            req.push_str("Host: localhost\r\n");
            req.push_str(&format!("Authorization: Bearer {token}\r\n"));
            req.push_str("Content-Type: application/json\r\n");
            if content_length > 0 {
                req.push_str(&format!("Content-Length: {content_length}\r\n"));
            }
            req.push_str("Connection: close\r\n\r\n");
            req.push_str(&body_str);

            stream
                .write_all(req.as_bytes())
                .context("Failed to write to daemon socket")?;
            stream.flush().context("Failed to flush")?;

            let mut buf = Vec::new();
            stream
                .read_to_end(&mut buf)
                .context("Failed to read daemon response")?;

            // Split headers from body
            let body_bytes = if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                &buf[pos + 4..]
            } else {
                &buf
            };

            serde_json::from_slice(body_bytes).context("Failed to parse daemon response as JSON")
        }
    }

    /// GET request to a daemon endpoint.
    pub fn get(&self, path: &str) -> Result<JsonValue> {
        self.send_request("GET", path, None)
    }

    /// POST request to a daemon endpoint.
    pub fn post(&self, path: &str, body: JsonValue) -> Result<JsonValue> {
        self.send_request("POST", path, Some(body))
    }

    /// Check if the daemon is reachable.
    pub fn is_reachable(&self) -> bool {
        self.get("/health").is_ok()
    }
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
