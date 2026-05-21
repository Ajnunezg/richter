//! Row types for the Richter SQLite database.
//!
//! Each struct maps to a table row via `sqlx::FromRow`. String columns that
//! represent typed enums are stored as text and parsed on access via helper
//! methods (e.g. `RunRow::classification()`, `RunRow::status()`).

use crate::models::{CommandClass, EventKind, ResourceClass, RunStatus};

/// A row from the `runs` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RunRow {
    /// Primary key.
    pub id: String,
    /// FK to repositories.
    pub repo_id: String,
    /// The raw command string.
    pub command: String,
    /// Classification label (build, test, lint, etc.) stored as text.
    #[sqlx(rename = "classification")]
    pub classification_str: String,
    /// Content-addressable fingerprint of the command + context.
    pub fingerprint: String,
    /// Current lifecycle status stored as text.
    #[sqlx(rename = "status")]
    pub status_str: String,
    /// Exit code (None while running).
    pub exit_code: Option<i32>,
    /// ISO 8601 timestamp when execution began.
    pub started_at: Option<String>,
    /// ISO 8601 timestamp when execution completed.
    pub completed_at: Option<String>,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: Option<i64>,
    /// Whether this run was served from cache.
    pub is_cached: i32,
    /// Resource class for scheduling stored as text.
    #[sqlx(rename = "resource_class")]
    pub resource_class_str: String,
    /// Path to captured stdout log.
    pub output_path: Option<String>,
    /// Path to captured stderr log.
    pub error_path: Option<String>,
}

impl RunRow {
    /// Parse the classification string into a typed enum.
    pub fn classification(&self) -> CommandClass {
        self.classification_str
            .parse()
            .unwrap_or(CommandClass::Unknown)
    }

    /// Parse the status string into a typed enum.
    pub fn status(&self) -> RunStatus {
        self.status_str.parse().unwrap_or(RunStatus::Queued)
    }

    /// Parse the resource class string into a typed enum.
    pub fn resource_class(&self) -> ResourceClass {
        self.resource_class_str
            .parse()
            .unwrap_or(ResourceClass::Unknown)
    }

    /// Whether the run was served from cache.
    pub fn is_cached(&self) -> bool {
        self.is_cached != 0
    }
}

/// A row from the `events` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EventRow {
    /// Primary key.
    pub id: String,
    /// Kind of event stored as text.
    #[sqlx(rename = "event_type")]
    pub event_type_str: String,
    /// Optional FK to runs.
    pub run_id: Option<String>,
    /// Optional FK to repositories.
    pub repo_id: Option<String>,
    /// Optional FK to agents.
    pub agent_id: Option<String>,
    /// Severity label.
    pub severity: Option<String>,
    /// Short title.
    pub title: String,
    /// Optional human-readable summary.
    pub summary: Option<String>,
    /// Optional detailed payload (JSON).
    pub details: Option<String>,
    /// Importance score.
    pub importance: i32,
    /// Whether this should trigger a notification (stored as integer).
    pub should_notify: i32,
    /// ISO 8601 timestamp.
    pub created_at: String,
}

impl EventRow {
    /// Parse the event type string into a typed enum.
    pub fn event_type(&self) -> EventKind {
        self.event_type_str.parse().unwrap_or(EventKind::Info)
    }

    /// Whether this should trigger a notification.
    pub fn should_notify(&self) -> bool {
        self.should_notify != 0
    }
}

/// A row from the `important_events` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ImportantEventRow {
    /// Primary key.
    pub id: String,
    /// FK to the underlying event.
    pub event_id: String,
    /// Importance score.
    pub importance: i32,
    /// Category label.
    pub category: Option<String>,
    /// Recommended action text.
    pub recommended_action: Option<String>,
    /// Whether the user acknowledged this event (stored as integer).
    pub acknowledged: i32,
    /// ISO 8601 timestamp.
    pub created_at: String,
}

impl ImportantEventRow {
    /// Whether this event was acknowledged.
    pub fn acknowledged(&self) -> bool {
        self.acknowledged != 0
    }
}

/// A row from the `run_cache` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CacheEntryRow {
    /// Primary key.
    pub id: String,
    /// Content fingerprint for cache lookup.
    pub fingerprint: String,
    /// FK to the run that produced this result.
    pub run_id: String,
    /// Cached exit code.
    pub exit_code: i32,
    /// Path to cached output log.
    pub output_path: Option<String>,
    /// ISO 8601 timestamp when cached.
    pub cached_at: String,
    /// ISO 8601 timestamp when this entry expires.
    pub expires_at: Option<String>,
}

/// A row from the `repositories` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RepoRow {
    /// Primary key.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Absolute path to the repository root.
    pub root: String,
    /// Current branch name.
    pub branch: Option<String>,
    /// HEAD commit SHA.
    pub head_sha: Option<String>,
    /// Whether the working tree has uncommitted changes (stored as integer).
    pub is_dirty: i32,
    /// Upstream remote URL.
    pub upstream: Option<String>,
    /// ISO 8601 timestamp of creation.
    pub created_at: String,
    /// ISO 8601 timestamp of last update.
    pub updated_at: String,
}

impl RepoRow {
    /// Whether the working tree has uncommitted changes.
    pub fn is_dirty(&self) -> bool {
        self.is_dirty != 0
    }
}

/// A row from the `agents` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AgentRow {
    /// Primary key.
    pub id: String,
    /// Agent program name (claude, codex, aider, etc.).
    pub agent_type: String,
    /// Optional human-readable name.
    pub name: Option<String>,
    /// Working directory.
    pub cwd: Option<String>,
    /// FK to repositories.
    pub repo_id: Option<String>,
    /// FK to worktrees.
    pub worktree_id: Option<String>,
    /// Currently executing command.
    pub active_command: Option<String>,
    /// ISO 8601 timestamp of last activity.
    pub last_seen_at: String,
    /// JSON-encoded metadata map.
    pub metadata: Option<String>,
}

/// A row from the `leases` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LeaseRow {
    /// Primary key.
    pub id: String,
    /// FK to the agent holding the lease.
    pub agent_id: String,
    /// The path being claimed.
    pub path: String,
    /// FK to the repository.
    pub repo_id: String,
    /// Lease duration in seconds.
    pub ttl_seconds: i64,
    /// ISO 8601 timestamp when acquired.
    pub acquired_at: String,
    /// ISO 8601 timestamp when the lease expires.
    pub expires_at: String,
    /// ISO 8601 timestamp when released (None = still active).
    pub released_at: Option<String>,
}

/// A row from the `mobile_devices` table.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MobileDeviceRow {
    /// Primary key (e.g. "mob_a1b2c3d4e5f6").
    pub id: String,
    /// Human-readable device name.
    pub display_name: String,
    /// Platform: "ios", "android", etc.
    pub platform: String,
    /// Base64-encoded Ed25519 public key.
    pub device_public_key: String,
    /// JSON array of scope strings.
    pub scopes_json: String,
    /// ISO 8601 timestamp when registered.
    pub created_at: String,
    /// ISO 8601 timestamp of last API call.
    pub last_seen_at: String,
    /// ISO 8601 timestamp when revoked (None = active).
    pub revoked_at: Option<String>,
    /// Human-readable revocation reason.
    pub revocation_reason: Option<String>,
    /// Whether push notifications are enabled (0/1).
    pub push_enabled: i32,
    /// Whether relay is enabled (0/1).
    pub relay_enabled: i32,
    /// App version string.
    pub app_version: Option<String>,
    /// OS version string.
    pub os_version: Option<String>,
}

impl MobileDeviceRow {
    /// Whether the device has push notifications enabled.
    pub fn push_enabled(&self) -> bool {
        self.push_enabled != 0
    }

    /// Whether the device has relay enabled.
    pub fn relay_enabled(&self) -> bool {
        self.relay_enabled != 0
    }
}
