//! Richter MCP server: exposes Richter's capabilities as MCP tools and resources
//! for AI coding agents (Codex, Claude Code, Droid, Forge Code, Kimi, MiniMax, etc.).
//!
//! Supports both stdio and local HTTP/streaming transports.

pub mod resources;
pub mod server;
pub mod tools;
pub mod transport;
pub mod daemon;
