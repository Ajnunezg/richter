//! MCP tool implementations for the Richter MCP server.
//!
//! Each function corresponds to an MCP tool exposed to AI coding agents.
//! Tools return structured JSON output and handle errors gracefully without
//! panicking.
//!
//! Module structure:
//! - `types` — input/output DTOs
//! - `registry` — tool registration and JSON schemas
//! - `handlers` — per-tool async handler functions and dispatch

pub mod handlers;
pub mod registry;
pub mod types;

pub use handlers::{dispatch_tool, ToolContext};
pub use registry::all_tools;
pub use registry::RegisteredTool;
