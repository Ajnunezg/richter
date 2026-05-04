//! MCP server implementation for Richter.
//!
//! Wraps the MCP protocol lifecycle: initialize, list tools/resources,
//! handle tool calls and resource reads, and shut down cleanly.
//!
//! The server is transport-agnostic — it accepts any `Transport` and
//! processes a standard JSON-RPC message loop.

pub mod protocol;

pub use crate::transport::InProcessPeer;
pub use protocol::run_inprocess_server;
pub use protocol::McpServer;
pub use protocol::McpServerConfig;
