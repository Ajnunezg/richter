//! Transport layer abstraction for the Richter MCP server.
//!
//! Provides three transport modes:
//! - **Stdio transport**: reads JSON-RPC messages from stdin, writes to stdout.
//!   stdout is strictly reserved for JSON-RPC; all logging goes to stderr.
//! - **HTTP transport**: axum-based SSE (Server-Sent Events) endpoint.
//! - **In-process transport**: for embedding the MCP server inside the daemon.
//!
//! All transports enforce that stdout is only for JSON-RPC in stdio mode.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Maximum size in bytes for a single JSON-RPC message over stdio.
const MAX_MESSAGE_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

/// A parsed JSON-RPC 2.0 message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcEnvelope {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error structure per the MCP/JSON-RPC 2.0 spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Abstraction over MCP transport backends.
///
/// Implementors provide a way to receive incoming JSON-RPC messages
/// and send outgoing JSON-RPC responses/notifications.
#[async_trait::async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Receive the next incoming JSON-RPC message.
    async fn recv(&mut self) -> Result<Option<JsonRpcEnvelope>>;

    /// Send a JSON-RPC message (response or notification).
    async fn send(&mut self, message: &JsonRpcEnvelope) -> Result<()>;

    /// Shut down the transport cleanly.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// Stdio-based MCP transport.
///
/// # Critical invariant
///
/// stdout is **reserved exclusively for JSON-RPC messages**. All logging,
/// diagnostics, and debugging output MUST go to stderr. Mixing non-JSON-RPC
/// output on stdout will corrupt the MCP protocol stream and cause client
/// disconnections or parse errors.
pub struct StdioTransport {
    /// Channel sender for outgoing JSON-RPC messages to be written to stdout.
    tx: mpsc::Sender<JsonRpcEnvelope>,
    /// Channel receiver for incoming JSON-RPC messages read from stdin.
    rx: mpsc::Receiver<JsonRpcEnvelope>,
    /// Join handle for the stdin reader task.
    stdin_handle: Option<tokio::task::JoinHandle<()>>,
    /// Join handle for the stdout writer task.
    stdout_handle: Option<tokio::task::JoinHandle<()>>,
}

impl StdioTransport {
    /// Create a new stdio transport.
    ///
    /// Spawns two background tasks:
    /// - A **stdin reader** that parses JSON-RPC lines from stdin and forwards
    ///   them to the receiver channel.
    /// - A **stdout writer** that reads from the sender channel and writes
    ///   JSON-RPC messages to stdout, one JSON value per line.
    ///
    /// # Errors
    ///
    /// Returns an error if stdin cannot be opened.
    pub fn new() -> Result<Self> {
        let (incoming_tx, incoming_rx) = mpsc::channel::<JsonRpcEnvelope>(1024);
        let (outgoing_tx, mut outgoing_rx) = mpsc::channel::<JsonRpcEnvelope>(1024);

        // Spawn stdin reader on a blocking thread.
        let incoming = incoming_tx;
        let stdin_handle = tokio::task::spawn_blocking(move || {
            let stdin = std::io::stdin();
            let reader = BufReader::new(stdin);
            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        let trimmed = line.trim().to_owned();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if trimmed.len() > MAX_MESSAGE_SIZE {
                            error!(
                                size = trimmed.len(),
                                "JSON-RPC message exceeds size limit; dropping"
                            );
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcEnvelope>(&trimmed) {
                            Ok(envelope) => {
                                debug!(method = ?envelope.method, id = ?envelope.id, "recv JSON-RPC");
                                if incoming.blocking_send(envelope).is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to parse JSON-RPC message from stdin");
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "stdin read error; stopping reader");
                        break;
                    }
                }
            }
            info!("stdin reader task finished");
        });

        // Spawn stdout writer on a blocking thread.
        let stdout_handle = tokio::task::spawn_blocking(move || {
            let mut stdout = std::io::stdout();
            while let Some(message) = outgoing_rx.blocking_recv() {
                match serde_json::to_string(&message) {
                    Ok(line) => {
                        // Write one line of JSON-RPC to stdout.
                        // writeln! may panic on OOM but that's unrecoverable anyway.
                        if let Err(e) = writeln!(stdout, "{}", line) {
                            error!(error = %e, "stdout write error");
                        }
                        if let Err(e) = stdout.flush() {
                            error!(error = %e, "stdout flush error");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "failed to serialize JSON-RPC message");
                    }
                }
            }
            info!("stdout writer task finished");
        });

        Ok(Self {
            tx: outgoing_tx,
            rx: incoming_rx,
            stdin_handle: Some(stdin_handle),
            stdout_handle: Some(stdout_handle),
        })
    }
}

#[async_trait::async_trait]
impl Transport for StdioTransport {
    async fn recv(&mut self) -> Result<Option<JsonRpcEnvelope>> {
        match self.rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None),
        }
    }

    async fn send(&mut self, message: &JsonRpcEnvelope) -> Result<()> {
        self.tx
            .send(message.clone())
            .await
            .context("failed to send JSON-RPC message on stdout channel — backpressure")?;
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!("shutting down stdio transport");
        if let Some(handle) = self.stdin_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.stdout_handle.take() {
            handle.abort();
        }
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        if let Some(handle) = self.stdin_handle.take() {
            handle.abort();
        }
        if let Some(handle) = self.stdout_handle.take() {
            handle.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP (SSE) transport
// ---------------------------------------------------------------------------

/// HTTP-based MCP transport using Server-Sent Events (SSE).
///
/// This transport is intended for use when the MCP server is embedded inside
/// the Richter daemon's axum HTTP server. Clients connect to an SSE endpoint
/// and receive JSON-RPC messages as SSE events.
pub struct HttpTransport {
    /// Channel sender for pushing events to connected SSE clients.
    tx: tokio::sync::broadcast::Sender<Arc<String>>,
    /// Incoming message receiver (populated by axum handler).
    rx: mpsc::Receiver<JsonRpcEnvelope>,
    /// Incoming message sender half (given to axum handler).
    incoming_tx: mpsc::Sender<JsonRpcEnvelope>,
}

impl HttpTransport {
    /// Create a new HTTP transport.
    ///
    /// Returns the transport and a broadcast sender that can be cloned
    /// and passed to axum SSE handlers.
    pub fn new() -> (Self, tokio::sync::broadcast::Sender<Arc<String>>) {
        let (event_tx, _) = tokio::sync::broadcast::channel::<Arc<String>>(256);
        let (incoming_tx, incoming_rx) = mpsc::channel::<JsonRpcEnvelope>(1024);

        let transport = Self {
            tx: event_tx.clone(),
            rx: incoming_rx,
            incoming_tx,
        };

        (transport, event_tx)
    }

    /// Get a clone of the incoming message sender for axum handlers.
    pub fn incoming_sender(&self) -> mpsc::Sender<JsonRpcEnvelope> {
        self.incoming_tx.clone()
    }
}

#[async_trait::async_trait]
impl Transport for HttpTransport {
    async fn recv(&mut self) -> Result<Option<JsonRpcEnvelope>> {
        match self.rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None),
        }
    }

    async fn send(&mut self, message: &JsonRpcEnvelope) -> Result<()> {
        let payload =
            serde_json::to_string(message).context("failed to serialize JSON-RPC message")?;
        let _ = self.tx.send(Arc::new(payload));
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<()> {
        info!("shutting down HTTP transport");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// In-process transport (for daemon embedding)
// ---------------------------------------------------------------------------

/// In-process MCP transport for daemon embedding.
///
/// Uses memory channels to pass JSON-RPC messages between the MCP server
/// and the daemon's internal handler. This avoids any serialization to
/// stdio or network when the MCP server is co-located inside the daemon
/// process.
pub struct InProcessTransport {
    tx: mpsc::Sender<JsonRpcEnvelope>,
    rx: mpsc::Receiver<JsonRpcEnvelope>,
}

impl InProcessTransport {
    /// Create a paired in-process transport.
    ///
    /// Returns two halves:
    /// - The `InProcessTransport` that the MCP server uses.
    /// - An `InProcessPeer` that the daemon's internal handler uses.
    pub fn paired() -> (Self, InProcessPeer) {
        let (server_tx, peer_rx) = mpsc::channel::<JsonRpcEnvelope>(1024);
        let (peer_tx, server_rx) = mpsc::channel::<JsonRpcEnvelope>(1024);

        let transport = Self {
            tx: server_tx,
            rx: server_rx,
        };
        let peer = InProcessPeer {
            tx: peer_tx,
            rx: peer_rx,
        };

        (transport, peer)
    }
}

#[async_trait::async_trait]
impl Transport for InProcessTransport {
    async fn recv(&mut self) -> Result<Option<JsonRpcEnvelope>> {
        match self.rx.recv().await {
            Some(msg) => Ok(Some(msg)),
            None => Ok(None),
        }
    }

    async fn send(&mut self, message: &JsonRpcEnvelope) -> Result<()> {
        self.tx
            .send(message.clone())
            .await
            .context("failed to send message on in-process channel — backpressure")?;
        Ok(())
    }
}

/// The peer half of an in-process transport pair.
///
/// Used by the daemon's internal handler to communicate with the
/// co-located MCP server.
pub struct InProcessPeer {
    tx: mpsc::Sender<JsonRpcEnvelope>,
    rx: mpsc::Receiver<JsonRpcEnvelope>,
}

impl InProcessPeer {
    /// Receive a message from the MCP server.
    pub async fn recv(&mut self) -> Option<JsonRpcEnvelope> {
        self.rx.recv().await
    }

    /// Send a message to the MCP server.
    /// Returns error if the channel is full (backpressure).
    pub async fn send(&self, msg: JsonRpcEnvelope) -> Result<()> {
        self.tx
            .send(msg)
            .await
            .context("failed to send message to MCP server — backpressure")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a JSON-RPC success response for a given request id.
pub fn rpc_success(id: Option<serde_json::Value>, result: serde_json::Value) -> JsonRpcEnvelope {
    JsonRpcEnvelope {
        jsonrpc: "2.0".into(),
        id,
        method: None,
        params: None,
        result: Some(result),
        error: None,
    }
}

/// Build a JSON-RPC error response for a given request id.
pub fn rpc_error(id: Option<serde_json::Value>, code: i32, message: &str) -> JsonRpcEnvelope {
    JsonRpcEnvelope {
        jsonrpc: "2.0".into(),
        id,
        method: None,
        params: None,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data: None,
        }),
    }
}

/// Build a JSON-RPC notification (no `id` field).
pub fn rpc_notification(method: &str, params: serde_json::Value) -> JsonRpcEnvelope {
    JsonRpcEnvelope {
        jsonrpc: "2.0".into(),
        id: None,
        method: Some(method.into()),
        params: Some(params),
        result: None,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_success_has_correct_structure() {
        let resp = rpc_success(
            Some(serde_json::Value::Number(1.into())),
            serde_json::json!({"ok": true}),
        );
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.id.is_some());
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn rpc_error_has_correct_structure() {
        let resp = rpc_error(
            Some(serde_json::Value::Number(2.into())),
            -32601,
            "Method not found",
        );
        assert_eq!(resp.jsonrpc, "2.0");
        assert!(resp.id.is_some());
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        assert_eq!(resp.error.as_ref().map(|e| e.code), Some(-32601));
    }

    #[test]
    fn rpc_notification_has_no_id() {
        let notif = rpc_notification("notifications/initialized", serde_json::json!({}));
        assert_eq!(notif.jsonrpc, "2.0");
        assert!(notif.id.is_none());
        assert!(notif.result.is_none());
        assert!(notif.error.is_none());
        assert_eq!(notif.method.as_deref(), Some("notifications/initialized"));
    }

    #[test]
    fn json_rpc_envelope_roundtrip() {
        let original = rpc_success(
            Some(serde_json::Value::Number(42.into())),
            serde_json::json!({"data": "hello"}),
        );
        let json = serde_json::to_string(&original).unwrap();
        let parsed: JsonRpcEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(original.jsonrpc, parsed.jsonrpc);
        assert_eq!(original.id, parsed.id);
        assert_eq!(original.result, parsed.result);
    }

    #[test]
    fn rpc_error_parses_back() {
        let original = rpc_error(None, -32000, "Server error");
        let json = serde_json::to_string(&original).unwrap();
        let parsed: JsonRpcEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.error.as_ref().map(|e| e.code), Some(-32000));
        assert_eq!(
            parsed.error.as_ref().map(|e| &e.message),
            Some(&"Server error".to_string())
        );
    }

    #[test]
    fn in_process_transport_paired_send_recv() {
        let (mut transport, peer) = InProcessTransport::paired();

        let request = rpc_success(
            Some(serde_json::Value::Number(1.into())),
            serde_json::json!({"ping": "pong"}),
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            peer.send(request.clone()).await.unwrap();

            let received = transport.recv().await.unwrap();
            assert!(received.is_some());
            let msg = received.unwrap();
            assert_eq!(msg.id, request.id);
            assert_eq!(msg.result, request.result);
        });
    }
}
