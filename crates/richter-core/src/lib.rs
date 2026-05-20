//! Richter core: shared data model, SQLite persistence, git repository detection,
//! command classification, fingerprinting, config, and secrets redaction.
//!
//! This crate is the foundation — it has no dependency on the daemon, CLI, or MCP server.

pub mod classifier;
pub mod config;
pub mod crypto;
pub mod db;
pub mod error;
pub mod fingerprint;
pub mod git;
pub mod models;
pub mod redact;
pub mod retry;
