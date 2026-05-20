//! MCP protocol message handling: initialize, tools/list, tools/call,
//! resources/list, resources/read, and ping.
//!
//! The `McpServer` struct is transport-agnostic and processes a standard
//! JSON-RPC message loop.

use crate::resources::{self, ResourceContext};
use crate::tools::{self, ToolContext};
use crate::transport::{rpc_error, InProcessPeer, JsonRpcEnvelope, Transport};
use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use tracing::{debug, error, info, warn};

/// Known MCP protocol methods.
#[derive(Debug, Clone, PartialEq)]
enum McpMethod {
    Initialize,
    Initialized,
    ToolsList,
    ToolsCall,
    ResourcesList,
    ResourcesRead,
    Ping,
    Unknown(String),
}

impl McpMethod {
    fn from_str(s: &str) -> Self {
        match s {
            "initialize" => McpMethod::Initialize,
            "notifications/initialized" => McpMethod::Initialized,
            "tools/list" => McpMethod::ToolsList,
            "tools/call" => McpMethod::ToolsCall,
            "resources/list" => McpMethod::ResourcesList,
            "resources/read" => McpMethod::ResourcesRead,
            "ping" => McpMethod::Ping,
            other => McpMethod::Unknown(other.to_string()),
        }
    }
}

/// Server capabilities advertised during initialization.
#[derive(Debug, Clone, serde::Serialize)]
struct ServerCapabilities {
    tools: ToolCapabilities,
    resources: ResourceCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    experimental: Option<JsonValue>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ToolCapabilities {
    #[serde(rename = "listChanged")]
    list_changed: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
struct ResourceCapabilities {
    subscribe: bool,
    #[serde(rename = "listChanged")]
    list_changed: bool,
}

/// MCP server configuration.
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// Server name advertised to clients.
    pub name: String,
    /// Server version advertised to clients.
    pub version: String,
    /// Whether the daemon is available.
    pub daemon_available: bool,
    /// Optional daemon API URL for forwarding tool/resource requests.
    pub daemon_api_url: Option<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: "Richter".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            daemon_available: false,
            daemon_api_url: None,
        }
    }
}

/// The Richter MCP server.
///
/// Owns the transport and processes incoming JSON-RPC requests in a loop.
pub struct McpServer<T: Transport> {
    pub transport: T,
    config: McpServerConfig,
    tool_ctx: ToolContext,
    resource_ctx: ResourceContext,
    initialized: bool,
}

impl<T: Transport> McpServer<T> {
    /// Create a new MCP server with the given transport and configuration.
    pub fn new(transport: T, config: McpServerConfig) -> Self {
        let tool_ctx = ToolContext {
            daemon_api_url: config.daemon_api_url.clone(),
            daemon_available: config.daemon_available,
            version: config.version.clone(),
            #[cfg(unix)]
            daemon_client: {
                let socket = std::env::var("RICHTER_SOCKET").unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                    format!("{home}/.richter/daemon.sock")
                });
                let client = crate::daemon::DaemonApiClient::new(&socket);
                if client.is_reachable() {
                    Some(client)
                } else {
                    None
                }
            },
        };
        let resource_ctx = ResourceContext {
            daemon_available: config.daemon_available,
            version: config.version.clone(),
        };

        Self {
            transport,
            config,
            tool_ctx,
            resource_ctx,
            initialized: false,
        }
    }

    /// Run the server's main message loop.
    ///
    /// Blocks until the transport is closed or a fatal error occurs.
    pub async fn serve(&mut self) -> Result<()> {
        info!(
            name = %self.config.name,
            version = %self.config.version,
            "Richter MCP server starting"
        );

        while let Some(message) = self
            .transport
            .recv()
            .await
            .context("transport recv error")?
        {
            self.handle_message(message).await?;
        }

        info!("Richter MCP server: transport closed; shutting down");
        self.shutdown().await
    }

    /// Shut down the server and its transport.
    pub async fn shutdown(&mut self) -> Result<()> {
        info!("shutting down Richter MCP server");
        self.transport
            .shutdown()
            .await
            .context("transport shutdown failed")?;
        Ok(())
    }

    /// Drive one JSON-RPC message through the server and return the response.
    ///
    /// This is public so that integration tests in other crates can test
    /// the MCP protocol without accessing private fields.
    pub async fn drive_message(&mut self, msg: JsonRpcEnvelope) -> Result<()> {
        self.handle_message(msg).await
    }

    /// Handle a single incoming JSON-RPC message.
    async fn handle_message(&mut self, msg: JsonRpcEnvelope) -> Result<()> {
        let method_str = match msg.method.as_deref() {
            Some(m) => m,
            None => {
                warn!("received message with no method");
                return Ok(());
            }
        };

        let method = McpMethod::from_str(method_str);

        match method {
            McpMethod::Initialize => {
                self.handle_initialize(&msg).await?;
            }
            McpMethod::Initialized => {
                info!("client sent initialized notification");
            }
            McpMethod::ToolsList => {
                self.handle_tools_list(&msg).await?;
            }
            McpMethod::ToolsCall => {
                self.handle_tools_call(&msg).await?;
            }
            McpMethod::ResourcesList => {
                self.handle_resources_list(&msg).await?;
            }
            McpMethod::ResourcesRead => {
                self.handle_resources_read(&msg).await?;
            }
            McpMethod::Ping => {
                self.handle_ping(&msg).await?;
            }
            McpMethod::Unknown(name) => {
                warn!(method = %name, "unknown method");
                let error_resp = rpc_error(
                    msg.id.clone(),
                    -32601,
                    &format!("Method not found: {}", name),
                );
                self.transport.send(&error_resp).await?;
            }
        }

        Ok(())
    }

    /// Handle the `initialize` request.
    async fn handle_initialize(&mut self, msg: &JsonRpcEnvelope) -> Result<()> {
        info!("handling initialize request");

        let capabilities = make_capabilities();

        let result = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": capabilities,
            "serverInfo": {
                "name": self.config.name,
                "version": self.config.version,
            }
        });

        let response = jsonrpc_ok(msg.id.clone(), result);
        self.transport.send(&response).await?;
        self.initialized = true;
        Ok(())
    }

    /// Handle the `tools/list` request.
    async fn handle_tools_list(&mut self, msg: &JsonRpcEnvelope) -> Result<()> {
        debug!("handling tools/list request");

        let tools_list: Vec<JsonValue> = tools::all_tools()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();

        let response = jsonrpc_ok(msg.id.clone(), serde_json::json!({ "tools": tools_list }));
        self.transport.send(&response).await?;
        Ok(())
    }

    /// Handle the `tools/call` request.
    async fn handle_tools_call(&mut self, msg: &JsonRpcEnvelope) -> Result<()> {
        let params = match &msg.params {
            Some(p) => p,
            None => {
                let error_resp = rpc_error(msg.id.clone(), -32602, "Missing params");
                self.transport.send(&error_resp).await?;
                return Ok(());
            }
        };

        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => {
                let error_resp = rpc_error(msg.id.clone(), -32602, "Missing tool name");
                self.transport.send(&error_resp).await?;
                return Ok(());
            }
        };

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        match tools::dispatch_tool(&tool_name, &self.tool_ctx, arguments).await {
            Ok(result) => {
                let content = make_text_content(&result.to_string(), false);
                let response = jsonrpc_ok(msg.id.clone(), content);
                self.transport.send(&response).await?;
            }
            Err(e) => {
                error!(tool = %tool_name, error = %e, "tool call failed");
                let content = make_text_content(&format!("Tool execution error: {:#}", e), true);
                let response = jsonrpc_ok(msg.id.clone(), content);
                self.transport.send(&response).await?;
            }
        }

        Ok(())
    }

    /// Handle the `resources/list` request.
    async fn handle_resources_list(&mut self, msg: &JsonRpcEnvelope) -> Result<()> {
        debug!("handling resources/list request");

        let resources_list: Vec<JsonValue> = resources::all_resources()
            .iter()
            .map(|r| {
                serde_json::json!({
                    "uri": r.uri,
                    "name": r.name,
                    "description": r.description,
                    "mimeType": r.mime_type,
                })
            })
            .collect();

        let response = jsonrpc_ok(
            msg.id.clone(),
            serde_json::json!({ "resources": resources_list }),
        );
        self.transport.send(&response).await?;
        Ok(())
    }

    /// Handle the `resources/read` request.
    async fn handle_resources_read(&mut self, msg: &JsonRpcEnvelope) -> Result<()> {
        let params = match &msg.params {
            Some(p) => p,
            None => {
                let error_resp = rpc_error(msg.id.clone(), -32602, "Missing params");
                self.transport.send(&error_resp).await?;
                return Ok(());
            }
        };

        let uri = match params.get("uri").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => {
                let error_resp = rpc_error(msg.id.clone(), -32602, "Missing uri");
                self.transport.send(&error_resp).await?;
                return Ok(());
            }
        };

        match resources::dispatch_resource_read(uri, &self.resource_ctx).await {
            Ok(result) => {
                let content = serde_json::json!({
                    "contents": [
                        {
                            "uri": uri,
                            "mimeType": "application/json",
                            "text": result.to_string()
                        }
                    ]
                });
                let response = jsonrpc_ok(msg.id.clone(), content);
                self.transport.send(&response).await?;
            }
            Err(e) => {
                error!(uri = %uri, error = %e, "resource read failed");
                let error_resp = rpc_error(
                    msg.id.clone(),
                    -32603,
                    &format!("Resource read error: {:#}", e),
                );
                self.transport.send(&error_resp).await?;
            }
        }

        Ok(())
    }

    /// Handle the `ping` request.
    async fn handle_ping(&mut self, msg: &JsonRpcEnvelope) -> Result<()> {
        let response = jsonrpc_ok(msg.id.clone(), serde_json::json!({}));
        self.transport.send(&response).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

/// Run the MCP server over stdio.
///
/// This is the standard mode for MCP clients like Claude Desktop and Codex.
/// stdout is reserved for JSON-RPC; all logging goes to stderr.
pub async fn run_stdio_server(config: McpServerConfig) -> Result<()> {
    let transport =
        crate::transport::StdioTransport::new().context("failed to create stdio transport")?;
    let mut server = McpServer::new(transport, config);
    server.serve().await
}

/// Run the MCP server over in-process channels.
///
/// Returns the peer handle for the daemon to communicate with.
pub async fn run_inprocess_server(config: McpServerConfig) -> Result<InProcessPeer> {
    let (transport, peer) = crate::transport::InProcessTransport::paired();
    let mut server = McpServer::new(transport, config);

    tokio::spawn(async move {
        if let Err(e) = server.serve().await {
            error!(error = %e, "in-process MCP server exited with error");
        }
    });

    Ok(peer)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn make_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        tools: ToolCapabilities {
            list_changed: false,
        },
        resources: ResourceCapabilities {
            subscribe: false,
            list_changed: false,
        },
        experimental: None,
    }
}

fn jsonrpc_ok(id: Option<JsonValue>, result: JsonValue) -> JsonRpcEnvelope {
    JsonRpcEnvelope {
        jsonrpc: "2.0".into(),
        id,
        method: None,
        params: None,
        result: Some(result),
        error: None,
    }
}

fn make_text_content(text: &str, is_error: bool) -> JsonValue {
    let mut obj = serde_json::json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ]
    });
    if is_error {
        obj["isError"] = serde_json::Value::Bool(true);
    }
    obj
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::InProcessTransport;

    fn create_test_server() -> (McpServer<InProcessTransport>, InProcessPeer) {
        let (transport, peer) = InProcessTransport::paired();
        let config = McpServerConfig {
            name: "Richter-test".to_string(),
            version: "0.1.0-test".to_string(),
            daemon_available: false,
            daemon_api_url: None,
        };
        let server = McpServer::new(transport, config);
        (server, peer)
    }

    fn make_request(id: u64, method: &str, params: JsonValue) -> JsonRpcEnvelope {
        JsonRpcEnvelope {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::Value::Number(id.into())),
            method: Some(method.into()),
            params: Some(params),
            result: None,
            error: None,
        }
    }

    #[allow(dead_code)]
    async fn send_and_recv_one(
        server: &mut McpServer<InProcessTransport>,
        peer: &InProcessPeer,
        request: JsonRpcEnvelope,
    ) -> JsonRpcEnvelope {
        peer.send(request).await.unwrap();
        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();
        // We need a clone of peer to recv. The peer isn't Clone currently.
        // Let's restructure: return peer so caller can recv on it.
        unreachable!("use the integrated helper instead")
    }

    #[tokio::test]
    async fn initialize_returns_capabilities() {
        let (mut server, peer) = create_test_server();

        let request = make_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            }),
        );
        peer.send(request).await.unwrap();

        // Process the message.
        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        assert_eq!(response.jsonrpc, "2.0");
        let result = response.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "Richter-test");
    }

    #[tokio::test]
    async fn tools_list_returns_nine_tools() {
        let (mut server, peer) = create_test_server();

        let request = make_request(2, "tools/list", serde_json::json!({}));
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        let result = response.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 9);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"richter_status"));
        assert!(names.contains(&"richter_run_or_join"));
    }

    #[tokio::test]
    async fn resources_list_returns_four_resources() {
        let (mut server, peer) = create_test_server();

        let request = make_request(3, "resources/list", serde_json::json!({}));
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        let result = response.result.unwrap();
        let resources = result["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 4);
    }

    #[tokio::test]
    async fn tools_call_richter_status_works() {
        let (mut server, peer) = create_test_server();

        let request = make_request(
            4,
            "tools/call",
            serde_json::json!({
                "name": "richter_status",
                "arguments": {}
            }),
        );
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        let result = response.result.unwrap();
        let content = result["content"].as_array().unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("daemon_not_running"));
    }

    #[tokio::test]
    async fn tools_call_unknown_tool_returns_error() {
        let (mut server, peer) = create_test_server();

        let request = make_request(
            5,
            "tools/call",
            serde_json::json!({
                "name": "nonexistent_tool",
                "arguments": {}
            }),
        );
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        let result = response.result.unwrap();
        let content = result["content"].as_array().unwrap();
        let text = content[0]["text"].as_str().unwrap();
        assert!(text.contains("unknown_tool"));
    }

    #[tokio::test]
    async fn resources_read_global_status_works() {
        let (mut server, peer) = create_test_server();

        let request = make_request(
            6,
            "resources/read",
            serde_json::json!({
                "uri": "richter://global/status"
            }),
        );
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        let result = response.result.unwrap();
        let contents = result["contents"].as_array().unwrap();
        let text = contents[0]["text"].as_str().unwrap();
        assert!(text.contains("daemon_running"));
    }

    #[tokio::test]
    async fn ping_returns_empty_object() {
        let (mut server, peer) = create_test_server();

        let request = make_request(7, "ping", serde_json::json!({}));
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        assert!(response.result.is_some());
        assert_eq!(response.result.unwrap(), serde_json::json!({}));
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let (mut server, peer) = create_test_server();

        let request = make_request(8, "some/nonexistent", serde_json::json!({}));
        peer.send(request).await.unwrap();

        let msg = server.transport.recv().await.unwrap().unwrap();
        server.handle_message(msg).await.unwrap();

        let mut peer_owned = peer;
        let response = peer_owned.recv().await.unwrap();
        assert!(response.error.is_some());
        let err = response.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn mcp_method_from_str_all_cases() {
        assert_eq!(McpMethod::from_str("initialize"), McpMethod::Initialize);
        assert_eq!(
            McpMethod::from_str("notifications/initialized"),
            McpMethod::Initialized
        );
        assert_eq!(McpMethod::from_str("tools/list"), McpMethod::ToolsList);
        assert_eq!(McpMethod::from_str("tools/call"), McpMethod::ToolsCall);
        assert_eq!(
            McpMethod::from_str("resources/list"),
            McpMethod::ResourcesList
        );
        assert_eq!(
            McpMethod::from_str("resources/read"),
            McpMethod::ResourcesRead
        );
        assert_eq!(McpMethod::from_str("ping"), McpMethod::Ping);
        assert!(matches!(
            McpMethod::from_str("unknown/method"),
            McpMethod::Unknown(_)
        ));
    }

    #[test]
    fn default_mcp_server_config_values() {
        let config = McpServerConfig::default();
        assert_eq!(config.name, "Richter");
        assert!(!config.version.is_empty());
        assert!(!config.daemon_available);
        assert!(config.daemon_api_url.is_none());
    }
}
