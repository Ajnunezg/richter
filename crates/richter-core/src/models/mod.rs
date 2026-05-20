//! Shared data models for the Richter agent-control plane.
//!
//! Defines all core types used across the daemon, CLI, MCP server,
//! and macOS app: repositories, worktrees, agents, runs, events,
//! decisions, leases, and configuration. Every public type derives
//! `Serialize`/`Deserialize` and carries Rustdoc.

// ---------------------------------------------------------------------------
// Sub-modules
// ---------------------------------------------------------------------------

pub mod agent;
pub mod command;
pub mod event;
pub mod ids;
pub mod repo;
pub mod resource;
pub mod run;

// ---------------------------------------------------------------------------
// Macro —— must be at crate root so sub-modules can invoke it
// ---------------------------------------------------------------------------

/// Macro to define strongly-typed ID newtypes that wrap `Uuid`.
/// Provides `Deref<Target=Uuid>`, `Display`, `FromStr`, `Serialize`, `Deserialize`,
/// and conversions from/to `Uuid` without permitting accidental cross-type usage.
#[macro_export]
macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub uuid::Uuid);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(s).map(Self)
            }
        }

        impl std::ops::Deref for $name {
            type Target = uuid::Uuid;
            fn deref(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl From<uuid::Uuid> for $name {
            fn from(u: uuid::Uuid) -> Self {
                Self(u)
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(id: $name) -> uuid::Uuid {
                id.0
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Re-exports —— every previously-public item is still accessible from
// `richter_core::models::*`
// ---------------------------------------------------------------------------

pub use agent::*;
pub use command::*;
pub use event::*;
pub use ids::*;
pub use repo::*;
pub use resource::*;
pub use run::*;
