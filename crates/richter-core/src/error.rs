//! Typed error hierarchy for Richter.
//!
//! Uses `thiserror` for library-level errors across the core crate.
//! The daemon and CLI continue to use `anyhow` for application-level error handling.

use std::path::PathBuf;

/// Top-level error type for the richter-core library.
#[derive(Debug, thiserror::Error)]
pub enum RichterError {
    /// A configuration file could not be read or parsed.
    #[error("config error at `{path}`: {reason}")]
    Config {
        /// Path to the config file.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },

    /// Command classification failed.
    #[error("classification error for `{command}`: {reason}")]
    Classification {
        /// The command that could not be classified.
        command: String,
        /// Human-readable reason.
        reason: String,
    },

    /// Fingerprint computation failed.
    #[error("fingerprint error for `{command}`: {reason}")]
    Fingerprint {
        /// The command being fingerprinted.
        command: String,
        /// Human-readable reason.
        reason: String,
    },

    /// A requested entity was not found.
    #[error("{entity} not found: {id}")]
    NotFound {
        /// The type of entity (e.g. "Run", "Repo").
        entity: String,
        /// The identifier that was not found.
        id: String,
    },

    /// The system is in an invalid state for the requested operation.
    #[error("invalid state: {reason}")]
    InvalidState {
        /// Human-readable reason.
        reason: String,
    },

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A TOML parsing error.
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
}

impl RichterError {
    /// Create a not-found error.
    pub fn not_found(entity: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity: entity.into(),
            id: id.into(),
        }
    }
}

/// Result alias for core operations.
pub type Result<T> = std::result::Result<T, RichterError>;
