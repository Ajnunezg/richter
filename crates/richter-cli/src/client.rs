//! Local API client: communicates with the Richter daemon via a Unix domain socket.
//!
//! The client sends HTTP requests and receives HTTP responses.
//! It handles auth token injection and connection reuse.

use anyhow::{Context, Result};
use std::io::{BufReader, Read, Write};
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

    /// Send an HTTP request and return the response body bytes.
    /// `json` must contain a "method" field mapped to an HTTP endpoint.
    pub fn send_raw(&self, json: &str) -> Result<Vec<u8>> {
        // Always use a fresh connection — daemon closes after response.
        let mut stream = self.connect()?;

        let val: serde_json::Value =
            serde_json::from_str(json).context("Failed to parse request JSON")?;
        let method = val
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("health");

        let (http_method, path) = match method {
            "health" => ("GET", "/health"),
            "status" => ("GET", "/status"),
            "repos" => ("GET", "/repos"),
            "agents" => ("GET", "/agents"),
            "runs" => ("GET", "/runs"),
            "events" => ("GET", "/events"),
            "install_status" => ("GET", "/install_status"),
            "settings" => ("GET", "/settings"),
            "run" => ("POST", "/run_or_join"),
            _ => ("GET", "/health"),
        };

        let body = if http_method == "POST" {
            let params = val
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            serde_json::to_string(&params).context("Failed to serialize body")?
        } else {
            String::new()
        };

        let token = self.auth_token.as_deref().unwrap_or("");

        // Manually construct request with flush-left headers
        let mut req = String::new();
        req.push_str(&format!("{http_method} {path} HTTP/1.1\r\n"));
        req.push_str("Host: localhost\r\n");
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
        req.push_str("Content-Type: application/json\r\n");
        if !body.is_empty() {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        req.push_str("Connection: close\r\n\r\n");
        req.push_str(&body);

        stream
            .write_all(req.as_bytes())
            .context("Failed to write request")?;
        stream.flush().context("Failed to flush")?;

        // Read full response
        let mut buf = Vec::new();
        let mut reader = BufReader::new(&stream);
        reader
            .get_mut()
            .read_to_end(&mut buf)
            .context("Failed to read response")?;

        // Split headers from body at \r\n\r\n
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            Ok(buf[pos + 4..].to_vec())
        } else if buf.is_empty() {
            anyhow::bail!("Daemon closed connection with no response");
        } else {
            // No header separator — return raw
            Ok(buf)
        }
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

    pub fn check_health(&self) -> Result<()> {
        let req = serde_json::json!({"method": "health"});
        self.send_raw(&serde_json::to_string(&req)?)?;
        Ok(())
    }
}

/// Public helper for loading the auth token.
/// Used by mobile_pair which connects directly to the TCP mobile gateway.
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
}
