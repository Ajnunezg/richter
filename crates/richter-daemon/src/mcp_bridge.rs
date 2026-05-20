//! MCP server bridge for the Richter daemon.
//!
//! Provides daemon-side plumbing to expose tools and resources through the
//! Model Context Protocol (MCP). Wraps functionality that will eventually be
//! implemented by the `richter-mcp` crate, providing a bridge layer that
//! the daemon can use to register handlers and serve MCP clients.
//!
//! The bridge owns:
//! - Tool registration (list, describe, invoke)
//! - Resource registration (list, read)
//! - Health and status introspection
//! - Run streaming through MCP notifications

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

use crate::event_bus::EventBus;
use crate::run_manager::RunManager;
use crate::scheduler::Scheduler;

// ---------------------------------------------------------------------------
// MCP types (simplified subset; richter-mcp will expand)
// ---------------------------------------------------------------------------

/// An MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Unique tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,
}

/// An MCP resource definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// Unique resource URI (e.g. `richter://runs/active`).
    pub uri: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// MIME type.
    pub mime_type: Option<String>,
}

/// A tool invocation request from an MCP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Tool name to invoke.
    pub name: String,
    /// Tool arguments (JSON).
    pub arguments: serde_json::Value,
}

/// A tool invocation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    /// Whether the invocation succeeded.
    pub success: bool,
    /// Result content (structured JSON).
    pub content: serde_json::Value,
    /// Optional error message.
    pub error: Option<String>,
}

impl ToolCallResult {
    /// Create a successful result.
    pub fn ok(content: serde_json::Value) -> Self {
        Self {
            success: true,
            content,
            error: None,
        }
    }

    /// Create a failed result.
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            content: serde_json::json!({}),
            error: Some(msg.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool handler trait
// ---------------------------------------------------------------------------

/// A handler for a single MCP tool.
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    /// Return the tool definition.
    fn definition(&self) -> McpTool;

    /// Invoke the tool with arguments.
    async fn invoke(&self, args: serde_json::Value) -> ToolCallResult;
}

// ---------------------------------------------------------------------------
// Built-in tools
// ---------------------------------------------------------------------------

/// Lists all active runs.
struct ListRunsTool {
    run_manager: Arc<RunManager>,
}

#[async_trait::async_trait]
impl ToolHandler for ListRunsTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "list_runs".into(),
            description: "List all active runs in the Richter daemon".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> ToolCallResult {
        let runs = self.run_manager.active_runs();
        ToolCallResult::ok(serde_json::json!({
            "count": runs.len(),
            "run_ids": runs,
        }))
    }
}

/// Returns daemon health and status.
struct HealthTool {
    scheduler: Arc<Scheduler>,
}

#[async_trait::async_trait]
impl ToolHandler for HealthTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "daemon_health".into(),
            description: "Check the health and status of the Richter daemon".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> ToolCallResult {
        let active = self.scheduler.active_count();
        let queued = self.scheduler.queue_depth();
        let snap = self.scheduler.resource_snapshot();

        ToolCallResult::ok(serde_json::json!({
            "status": "running",
            "active_runs": active,
            "queued_runs": queued,
            "cpu_percent": snap.cpu_percent,
            "memory_percent": snap.memory_percent,
        }))
    }
}

/// Runs a command through the daemon's run-or-join machinery.
struct RunOrJoinTool {
    run_manager: Arc<RunManager>,
}

#[async_trait::async_trait]
impl ToolHandler for RunOrJoinTool {
    fn definition(&self) -> McpTool {
        McpTool {
            name: "run_or_join".into(),
            description:
                "Run a command through the Richter daemon, or join an existing equivalent run"
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "repo": { "type": "string", "description": "Repository path" },
                    "classification": { "type": "string", "description": "Command classification" }
                },
                "required": ["command"]
            }),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> ToolCallResult {
        let command = match args["command"].as_str() {
            Some(c) => c.to_string(),
            None => return ToolCallResult::err("Missing 'command' argument"),
        };
        let repo = args["repo"].as_str().unwrap_or(".").to_string();
        let classification = args["classification"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();

        let spec = crate::supervisor::RunSpec {
            command,
            repo: std::path::PathBuf::from(repo),
            classification: classification
                .parse()
                .unwrap_or(richter_core::models::CommandClass::Unknown),
            ..Default::default()
        };

        match self.run_manager.run_or_join(spec).await {
            Ok(outcome) => {
                let outcome_json = serde_json::to_value(&outcome).unwrap_or_default();
                ToolCallResult::ok(outcome_json)
            }
            Err(e) => ToolCallResult::err(format!("Failed to run command: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge
// ---------------------------------------------------------------------------

/// Manages MCP tool registration and invocation for the daemon.
///
/// Acts as a bridge between daemon capabilities and the MCP protocol.
pub struct McpBridge {
    /// Registered tool handlers by name.
    tools: HashMap<String, Box<dyn ToolHandler>>,
    /// Registered resources.
    resources: Vec<McpResource>,
    /// Event bus for notifications.
    event_bus: EventBus,
}

impl McpBridge {
    /// Create a new MCP bridge.
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            tools: HashMap::new(),
            resources: Vec::new(),
            event_bus,
        }
    }

    /// Create a bridge with all built-in tools registered.
    pub fn with_defaults(
        run_manager: Arc<RunManager>,
        scheduler: Arc<Scheduler>,
        event_bus: EventBus,
    ) -> Self {
        let mut bridge = Self::new(event_bus);
        bridge.register(Box::new(ListRunsTool {
            run_manager: run_manager.clone(),
        }));
        bridge.register(Box::new(HealthTool { scheduler }));
        bridge.register(Box::new(RunOrJoinTool { run_manager }));
        bridge
    }

    /// Register a tool handler.
    pub fn register(&mut self, handler: Box<dyn ToolHandler>) {
        let def = handler.definition();
        info!("Registered MCP tool: {}", def.name);
        self.tools.insert(def.name, handler);
    }

    /// Register a resource.
    pub fn add_resource(&mut self, resource: McpResource) {
        self.resources.push(resource);
    }

    /// List all registered tools.
    pub fn list_tools(&self) -> Vec<McpTool> {
        self.tools.values().map(|h| h.definition()).collect()
    }

    /// List all registered resources.
    pub fn list_resources(&self) -> Vec<McpResource> {
        self.resources.clone()
    }

    /// Invoke a tool by name.
    pub async fn invoke_tool(&self, name: &str, arguments: serde_json::Value) -> ToolCallResult {
        match self.tools.get(name) {
            Some(handler) => handler.invoke(arguments).await,
            None => ToolCallResult::err(format!("Unknown tool: {name}")),
        }
    }

    /// Read a resource by URI.
    pub fn read_resource(&self, uri: &str) -> Option<String> {
        // Dynamically generate resource content
        match uri {
            "richter://runs/active" => {
                // Would be populated from the run manager
                Some("[]".into())
            }
            "richter://health" => Some(serde_json::json!({"status": "running"}).to_string()),
            _ => self
                .resources
                .iter()
                .find(|r| r.uri == uri)
                .map(|_| "{}".into()),
        }
    }

    /// Subscribe to daemon events as MCP notifications.
    pub async fn stream_events(
        &self,
        filter: crate::event_bus::EventFilter,
    ) -> crate::event_bus::FilteredReceiver {
        self.event_bus.subscribe_filtered(filter)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bridge() -> McpBridge {
        McpBridge::new(EventBus::new())
    }

    #[tokio::test]
    async fn test_register_and_list_tools() {
        let mut bridge = make_test_bridge();

        struct DummyTool;
        #[async_trait::async_trait]
        impl ToolHandler for DummyTool {
            fn definition(&self) -> McpTool {
                McpTool {
                    name: "dummy".into(),
                    description: "A dummy tool".into(),
                    input_schema: serde_json::json!({}),
                }
            }
            async fn invoke(&self, _args: serde_json::Value) -> ToolCallResult {
                ToolCallResult::ok(serde_json::json!({"ok": true}))
            }
        }

        bridge.register(Box::new(DummyTool));
        let tools = bridge.list_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "dummy");
    }

    #[tokio::test]
    async fn test_invoke_unknown_tool() {
        let bridge = make_test_bridge();
        let result = bridge
            .invoke_tool("nonexistent", serde_json::json!({}))
            .await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Unknown tool"));
    }

    #[test]
    fn test_add_and_list_resources() {
        let mut bridge = make_test_bridge();
        bridge.add_resource(McpResource {
            uri: "richter://custom".into(),
            name: "Custom".into(),
            description: Some("A custom resource".into()),
            mime_type: Some("application/json".into()),
        });
        assert_eq!(bridge.list_resources().len(), 1);
    }

    #[test]
    fn test_read_resource_health() {
        let bridge = make_test_bridge();
        let content = bridge.read_resource("richter://health");
        assert!(content.is_some());
    }

    #[test]
    fn test_tool_call_result_ok() {
        let result = ToolCallResult::ok(serde_json::json!({"x": 1}));
        assert!(result.success);
        assert_eq!(result.content["x"], 1);
    }
}
