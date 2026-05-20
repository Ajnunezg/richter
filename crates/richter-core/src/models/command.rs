//! Command classification types used for scheduling and deduplication.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// CommandClass
// ---------------------------------------------------------------------------

/// Classification of a shell command for scheduling and deduplication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandClass {
    /// Build command (cargo build, make, cmake, etc.).
    Build,
    /// Test command (cargo test, pytest, jest, etc.).
    Test,
    /// Lint command (eslint, ruff, clippy, etc.).
    Lint,
    /// Type-check command (tsc, mypy, etc.).
    Typecheck,
    /// Formatter command (prettier, cargo fmt, etc.).
    Format,
    /// Dependency installation (npm install, pip install, etc.).
    Install,
    /// Dev server or watch mode.
    DevServer,
    /// Database or schema migration.
    Migration,
    /// Potentially destructive command (rm, drop, purge, etc.).
    Destructive,
    /// Unknown / passthrough command.
    Unknown,
}

impl std::fmt::Display for CommandClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CommandClass::Build => "build",
            CommandClass::Test => "test",
            CommandClass::Lint => "lint",
            CommandClass::Typecheck => "typecheck",
            CommandClass::Format => "format",
            CommandClass::Install => "install",
            CommandClass::DevServer => "dev_server",
            CommandClass::Migration => "migration",
            CommandClass::Destructive => "destructive",
            CommandClass::Unknown => "unknown",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for CommandClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "build" => Ok(CommandClass::Build),
            "test" => Ok(CommandClass::Test),
            "lint" => Ok(CommandClass::Lint),
            "typecheck" => Ok(CommandClass::Typecheck),
            "format" => Ok(CommandClass::Format),
            "install" => Ok(CommandClass::Install),
            "dev_server" => Ok(CommandClass::DevServer),
            "migration" => Ok(CommandClass::Migration),
            "destructive" => Ok(CommandClass::Destructive),
            "unknown" => Ok(CommandClass::Unknown),
            other => Err(format!("unknown CommandClass: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_class_display() {
        assert_eq!(CommandClass::Build.to_string(), "build");
        assert_eq!(CommandClass::Test.to_string(), "test");
        assert_eq!(CommandClass::Lint.to_string(), "lint");
        assert_eq!(CommandClass::Unknown.to_string(), "unknown");
    }

    #[test]
    fn test_serialize_command_class() {
        let json = serde_json::to_string(&CommandClass::Build).unwrap();
        assert_eq!(json, "\"build\"");
        let parsed: CommandClass = serde_json::from_str("\"test\"").unwrap();
        assert_eq!(parsed, CommandClass::Test);
    }
}
