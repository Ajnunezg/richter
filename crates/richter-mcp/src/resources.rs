//! MCP resource implementations for the Richter MCP server.
//!
//! Resources provide read-only access to Richter state via URI templates.
//! Each resource has a URI pattern and returns structured JSON content.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::debug;

// ---------------------------------------------------------------------------
// Resource output types
// ---------------------------------------------------------------------------

/// Content returned by the `richter://global/status` resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalStatusResource {
    pub daemon_running: bool,
    pub daemon_version: String,
    pub active_runs: usize,
    pub queued_runs: usize,
    pub cache_hits_today: u64,
    pub known_repos: usize,
    pub known_agents: usize,
    pub system_pressure: String,
}

/// Content returned by the `richter://repo/{repo_id}/status` resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoStatusResource {
    pub repo_id: String,
    pub repo_root: String,
    pub branch: String,
    pub head_sha: String,
    pub dirty: bool,
    pub active_agents: usize,
    pub active_runs: usize,
    pub queued_runs: usize,
    pub recent_important_events: usize,
}

/// Content returned by the `richter://run/{run_id}/summary` resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummaryResource {
    pub run_id: String,
    pub command: Vec<String>,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub cached: bool,
    pub subscriber_count: usize,
    pub repo_id: Option<String>,
    pub resource_class: String,
    pub log_path: Option<String>,
}

/// Content returned by the `richter://agent/{agent_id}/inbox` resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInboxResource {
    pub agent_id: String,
    pub messages: Vec<InboxMessage>,
    pub unread_count: usize,
}

/// A single message in an agent's inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMessage {
    pub message_id: String,
    pub importance: u8,
    pub title: String,
    pub body: String,
    pub occurred_at: String,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Resource registry
// ---------------------------------------------------------------------------

/// Represents a registered MCP resource with its URI template and metadata.
#[derive(Debug, Clone)]
pub struct RegisteredResource {
    /// The resource URI (may include path parameters in `{...}` notation).
    pub uri: &'static str,
    /// Human-readable name.
    pub name: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    /// MIME type of the resource content.
    pub mime_type: &'static str,
}

/// Return all registered Richter MCP resources.
pub fn all_resources() -> Vec<RegisteredResource> {
    vec![
        RegisteredResource {
            uri: "richter://global/status",
            name: "Global Status",
            description: "Read-only snapshot of the entire Richter system: daemon health, active/queued runs, cache stats, known repos and agents.",
            mime_type: "application/json",
        },
        RegisteredResource {
            uri: "richter://repo/{repo_id}/status",
            name: "Repository Status",
            description: "Read-only status for a specific repository: branch, dirty state, active agents, and run counts.",
            mime_type: "application/json",
        },
        RegisteredResource {
            uri: "richter://run/{run_id}/summary",
            name: "Run Summary",
            description: "Read-only summary of a specific run: command, timeline, exit code, cache status, subscribers, and log path.",
            mime_type: "application/json",
        },
        RegisteredResource {
            uri: "richter://agent/{agent_id}/inbox",
            name: "Agent Inbox",
            description: "Read-only inbox for a specific agent: unread messages with importance ratings and categories.",
            mime_type: "application/json",
        },
    ]
}

// ---------------------------------------------------------------------------
// Resource handler context
// ---------------------------------------------------------------------------

/// Context passed to resource handlers.
///
/// Similar to `ToolContext` — holds references to daemon state.
pub struct ResourceContext {
    /// Whether the daemon is available.
    pub daemon_available: bool,
    /// Version string.
    pub version: String,
}

impl Default for ResourceContext {
    fn default() -> Self {
        Self {
            daemon_available: false,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Resource handlers
// ---------------------------------------------------------------------------

/// Read the `richter://global/status` resource.
pub async fn read_global_status(ctx: &ResourceContext) -> Result<JsonValue> {
    debug!("read_global_status resource accessed");

    let resource = GlobalStatusResource {
        daemon_running: ctx.daemon_available,
        daemon_version: ctx.version.clone(),
        active_runs: 0,
        queued_runs: 0,
        cache_hits_today: 0,
        known_repos: 0,
        known_agents: 0,
        system_pressure: if ctx.daemon_available {
            "normal".to_string()
        } else {
            "unknown".to_string()
        },
    };

    serde_json::to_value(&resource).context("failed to serialize global status resource")
}

/// Read the `richter://repo/{repo_id}/status` resource.
pub async fn read_repo_status(ctx: &ResourceContext, repo_id: &str) -> Result<JsonValue> {
    debug!(repo_id = %repo_id, "read_repo_status resource accessed");

    let resource = RepoStatusResource {
        repo_id: repo_id.to_string(),
        repo_root: if ctx.daemon_available {
            "unknown — daemon stub".to_string()
        } else {
            "daemon offline".to_string()
        },
        branch: "unknown".to_string(),
        head_sha: "unknown".to_string(),
        dirty: false,
        active_agents: 0,
        active_runs: 0,
        queued_runs: 0,
        recent_important_events: 0,
    };

    serde_json::to_value(&resource).context("failed to serialize repo status resource")
}

/// Read the `richter://run/{run_id}/summary` resource.
pub async fn read_run_summary(ctx: &ResourceContext, run_id: &str) -> Result<JsonValue> {
    debug!(run_id = %run_id, "read_run_summary resource accessed");

    let resource = RunSummaryResource {
        run_id: run_id.to_string(),
        command: if ctx.daemon_available {
            vec!["unknown — daemon stub".to_string()]
        } else {
            vec!["daemon offline".to_string()]
        },
        status: "unknown".to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        exit_code: None,
        cached: false,
        subscriber_count: 0,
        repo_id: None,
        resource_class: "unknown".to_string(),
        log_path: None,
    };

    serde_json::to_value(&resource).context("failed to serialize run summary resource")
}

/// Read the `richter://agent/{agent_id}/inbox` resource.
pub async fn read_agent_inbox(_ctx: &ResourceContext, agent_id: &str) -> Result<JsonValue> {
    debug!(agent_id = %agent_id, "read_agent_inbox resource accessed");

    let resource = AgentInboxResource {
        agent_id: agent_id.to_string(),
        messages: vec![],
        unread_count: 0,
    };

    serde_json::to_value(&resource).context("failed to serialize agent inbox resource")
}

/// Dispatch a resource read by URI.
///
/// Parses the URI template parameters and delegates to the appropriate handler.
pub async fn dispatch_resource_read(uri: &str, ctx: &ResourceContext) -> Result<JsonValue> {
    // Exact match resources.
    if uri == "richter://global/status" {
        return read_global_status(ctx).await;
    }

    // Templated resources.
    if let Some(repo_id) = extract_param(uri, "richter://repo/", "/status") {
        return read_repo_status(ctx, repo_id).await;
    }

    if let Some(run_id) = extract_param(uri, "richter://run/", "/summary") {
        return read_run_summary(ctx, run_id).await;
    }

    if let Some(agent_id) = extract_param(uri, "richter://agent/", "/inbox") {
        return read_agent_inbox(ctx, agent_id).await;
    }

    Ok(serde_json::json!({
        "error": "unknown_resource",
        "message": format!("Resource '{}' is not recognized.", uri),
    }))
}

/// Extract a path parameter between a prefix and suffix.
///
/// For example, `extract_param("richter://repo/my-repo/status", "richter://repo/", "/status")`
/// returns `Some("my-repo")`.
fn extract_param<'a>(uri: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    let rest = uri.strip_prefix(prefix)?;
    let param = rest.strip_suffix(suffix)?;
    if param.is_empty() {
        None
    } else {
        Some(param)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_resources_returns_four() {
        let resources = all_resources();
        assert_eq!(resources.len(), 4);
    }

    #[test]
    fn all_resources_have_name_and_description() {
        for res in all_resources() {
            assert!(!res.uri.is_empty());
            assert!(!res.name.is_empty());
            assert!(!res.description.is_empty());
            assert_eq!(res.mime_type, "application/json");
        }
    }

    #[test]
    fn extract_param_valid() {
        assert_eq!(
            extract_param("richter://repo/abc/status", "richter://repo/", "/status"),
            Some("abc")
        );
        assert_eq!(
            extract_param("richter://run/42-x/summary", "richter://run/", "/summary"),
            Some("42-x")
        );
        assert_eq!(
            extract_param("richter://agent/bot7/inbox", "richter://agent/", "/inbox"),
            Some("bot7")
        );
    }

    #[test]
    fn extract_param_invalid() {
        assert_eq!(
            extract_param("richter://repo//status", "richter://repo/", "/status"),
            None
        );
        assert_eq!(
            extract_param("richter://other/abc/status", "richter://repo/", "/status"),
            None
        );
    }

    #[tokio::test]
    async fn dispatch_exact_global_status() {
        let ctx = ResourceContext::default();
        let result = dispatch_resource_read("richter://global/status", &ctx)
            .await
            .unwrap();
        assert_eq!(result["daemon_running"], false);
        assert!(!result["daemon_version"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_templated_repo_status() {
        let ctx = ResourceContext::default();
        let result = dispatch_resource_read("richter://repo/test-repo/status", &ctx)
            .await
            .unwrap();
        assert_eq!(result["repo_id"], "test-repo");
    }

    #[tokio::test]
    async fn dispatch_templated_run_summary() {
        let ctx = ResourceContext::default();
        let result = dispatch_resource_read("richter://run/run-001/summary", &ctx)
            .await
            .unwrap();
        assert_eq!(result["run_id"], "run-001");
    }

    #[tokio::test]
    async fn dispatch_templated_agent_inbox() {
        let ctx = ResourceContext::default();
        let result = dispatch_resource_read("richter://agent/my-agent/inbox", &ctx)
            .await
            .unwrap();
        assert_eq!(result["agent_id"], "my-agent");
        assert_eq!(result["unread_count"], 0);
    }

    #[tokio::test]
    async fn dispatch_unknown_resource_returns_error() {
        let ctx = ResourceContext::default();
        let result = dispatch_resource_read("richter://nonexistent/path", &ctx)
            .await
            .unwrap();
        assert_eq!(result["error"], "unknown_resource");
    }
}
