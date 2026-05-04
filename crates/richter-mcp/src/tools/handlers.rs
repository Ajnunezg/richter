//! Tool handler implementations and dispatch logic.
//!
//! Each tool function takes a `ToolContext` and a typed input struct,
//! performs the requested operation (or returns a graceful stub when the
//! daemon is offline), and returns a JSON value for the MCP response.

use super::types::*;
use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use tracing::{debug, info, warn};

/// Handler context passed to each tool implementation.
///
/// In a real deployment this would hold references to the daemon API client,
/// database handle, etc. It is designed to be augmented as the stack matures.
pub struct ToolContext {
    /// Optional daemon API base URL for remote queries.
    pub daemon_api_url: Option<String>,
    /// Whether the daemon is locally available and responsive.
    pub daemon_available: bool,
    /// Version string for display in status responses.
    pub version: String,
    /// Daemon API client for real queries when available.
    #[cfg(unix)]
    pub daemon_client: Option<crate::daemon::DaemonApiClient>,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            daemon_api_url: None,
            daemon_available: false,
            version: env!("CARGO_PKG_VERSION").to_string(),
            #[cfg(unix)]
            daemon_client: None,
        }
    }
}

/// Query the daemon for global status.
pub async fn richter_status(ctx: &ToolContext, _input: StatusInput) -> Result<JsonValue> {
    info!("richter_status called");
    #[cfg(unix)]
    {
        if let Some(client) = &ctx.daemon_client {
            match client.get("/status") {
                Ok(status) => return Ok(status),
                Err(e) => {
                    warn!("Daemon status query failed: {e}");
                }
            }
        }
    }
    if !ctx.daemon_available {
        return Ok(daemon_offline_response("richter_status"));
    }

    let status = GlobalStatus {
        daemon: DaemonStatus {
            running: true,
            uptime_seconds: 0,
            version: ctx.version.clone(),
        },
        active_runs: 0,
        queued_runs: 0,
        cache_hits_today: 0,
        duplicate_work_saved_estimate: "unknown — daemon not fully connected".to_string(),
        system_pressure: SystemPressure {
            cpu_percent: 0.0,
            memory_percent: 0.0,
            disk_percent: 0.0,
            level: "unknown".to_string(),
        },
        known_repos: 0,
        known_agents: 0,
    };

    serde_json::to_value(&status).context("failed to serialize global status")
}

/// Query the daemon for per-repo status.
pub async fn richter_repo_status(ctx: &ToolContext, input: RepoStatusInput) -> Result<JsonValue> {
    debug!(repo_id = %input.repo_id, "richter_repo_status called");
    if !ctx.daemon_available {
        return Ok(daemon_offline_response("richter_repo_status"));
    }

    let status = RepoStatus {
        repo_id: input.repo_id.clone(),
        repo_root: "unknown — daemon query stub".to_string(),
        branch: "unknown".to_string(),
        head_sha: "unknown".to_string(),
        dirty: false,
        active_agents: 0,
        active_runs: vec![],
        queued_runs: 0,
        recent_events: vec![],
    };

    serde_json::to_value(&status).context("failed to serialize repo status")
}

/// Submit a command through Richter.
pub async fn richter_run_or_join(ctx: &ToolContext, input: RunOrJoinInput) -> Result<JsonValue> {
    info!(
        command = ?input.command,
        cwd = ?input.cwd,
        label = ?input.label,
        "richter_run_or_join called"
    );

    #[cfg(unix)]
    {
        if let Some(client) = &ctx.daemon_client {
            let body = serde_json::json!({
                "command": input.command.join(" "),
                "repo": input.cwd.as_deref().unwrap_or("."),
                "classification": input.label.as_deref().unwrap_or("unknown"),
            });
            return match client.post("/run_or_join", body) {
                Ok(result) => Ok(result),
                Err(e) => {
                    warn!("Daemon run_or_join failed: {e}");
                    Ok(serde_json::json!({
                        "error": "daemon_request_failed",
                        "message": format!("{e}"),
                        "fallback": "run_directly"
                    }))
                }
            };
        }
    }

    if !ctx.daemon_available {
        return Ok(serde_json::json!({
            "error": "daemon_not_running",
            "message": "The Richter daemon is not currently running. Commands will execute directly without coordination.",
            "fallback": "run_directly"
        }));
    }

    let result = RunOrJoinResult {
        disposition: "unknown".to_string(),
        run_id: uuid::Uuid::new_v4().to_string(),
        message: "Run submitted via MCP; check run status for outcome.".to_string(),
        exit_code: None,
        cached: false,
        subscriber_count: 1,
    };

    serde_json::to_value(&result).context("failed to serialize run-or-join result")
}

/// List active runs.
pub async fn richter_active_runs(ctx: &ToolContext, input: ActiveRunsInput) -> Result<JsonValue> {
    debug!(repo_id = ?input.repo_id, limit = input.limit, "richter_active_runs called");
    #[cfg(unix)]
    {
        if let Some(client) = &ctx.daemon_client {
            match client.get("/runs") {
                Ok(runs) => return Ok(runs),
                Err(e) => {
                    warn!("Daemon runs query failed: {e}");
                }
            }
        }
    }
    if !ctx.daemon_available {
        return Ok(daemon_offline_with_empty("runs"));
    }

    let runs: Vec<RunSummary> = vec![];
    let total = runs.len();
    Ok(serde_json::json!({ "runs": runs, "total": total }))
}

/// Retrieve recent important events.
pub async fn richter_recent_important_events(
    ctx: &ToolContext,
    input: RecentEventsInput,
) -> Result<JsonValue> {
    debug!(
        limit = input.limit,
        "richter_recent_important_events called"
    );
    if !ctx.daemon_available {
        return Ok(daemon_offline_with_empty("events"));
    }

    let events: Vec<EventSummary> = vec![];
    let total = events.len();
    Ok(serde_json::json!({ "events": events, "total": total }))
}

/// Claim file paths.
pub async fn richter_claim_paths(ctx: &ToolContext, input: ClaimPathsInput) -> Result<JsonValue> {
    info!(
        paths = ?input.paths,
        agent_id = %input.agent_id,
        ttl = ?input.ttl,
        "richter_claim_paths called"
    );

    if !ctx.daemon_available {
        return Ok(serde_json::json!({
            "error": "daemon_not_running",
            "message": "The Richter daemon is not currently running. Claims are not available.",
            "claimed": [],
            "conflicts": input.paths,
        }));
    }

    let result = ClaimResult {
        claimed: input.paths.clone(),
        conflicts: vec![],
        expires_at: input.ttl.map(|t| format!("TTL: {}", t)),
    };

    serde_json::to_value(&result).context("failed to serialize claim result")
}

/// Release file paths.
pub async fn richter_release_paths(
    ctx: &ToolContext,
    input: ReleasePathsInput,
) -> Result<JsonValue> {
    info!(paths = ?input.paths, agent_id = %input.agent_id, "richter_release_paths called");

    if !ctx.daemon_available {
        return Ok(daemon_offline_with_empty("released"));
    }

    let result = ReleaseResult {
        released: input.paths.clone(),
    };

    serde_json::to_value(&result).context("failed to serialize release result")
}

/// Explain a scheduling decision.
pub async fn richter_explain_decision(
    ctx: &ToolContext,
    input: ExplainDecisionInput,
) -> Result<JsonValue> {
    debug!(decision_id = %input.decision_id, "richter_explain_decision called");

    if !ctx.daemon_available {
        return Ok(serde_json::json!({
            "error": "daemon_not_running",
            "message": "The Richter daemon is not currently running.",
            "decision_id": input.decision_id,
            "reason": "unknown — daemon offline"
        }));
    }

    let explanation = DecisionExplanation {
        decision_id: input.decision_id.clone(),
        command: "unknown — stub".to_string(),
        disposition: "unknown".to_string(),
        reason: "Decision explanation not available — daemon stub.".to_string(),
        fingerprint: "unknown".to_string(),
        decided_at: chrono::Utc::now().to_rfc3339(),
        confidence: 0.0,
    };

    serde_json::to_value(&explanation).context("failed to serialize decision explanation")
}

/// Get run summary.
pub async fn richter_get_run_summary(
    ctx: &ToolContext,
    input: GetRunSummaryInput,
) -> Result<JsonValue> {
    debug!(run_id = %input.run_id, "richter_get_run_summary called");

    if !ctx.daemon_available {
        return Ok(serde_json::json!({
            "error": "daemon_not_running",
            "message": "The Richter daemon is not currently running.",
            "run_id": input.run_id,
        }));
    }

    let summary = RunSummary {
        run_id: input.run_id.clone(),
        command: vec!["unknown — stub".to_string()],
        status: "unknown".to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        subscriber_count: 0,
        repo_id: None,
        resource_class: "unknown".to_string(),
    };

    serde_json::to_value(&summary).context("failed to serialize run summary")
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a tool call by name to the appropriate handler.
///
/// Returns the JSON result or an error message. Unknown tools return
/// a structured error rather than panicking.
pub async fn dispatch_tool(
    name: &str,
    ctx: &ToolContext,
    arguments: JsonValue,
) -> Result<JsonValue> {
    match name {
        "richter_status" => {
            let input: StatusInput = serde_json::from_value(arguments)
                .context("failed to parse richter_status arguments")?;
            richter_status(ctx, input).await
        }
        "richter_repo_status" => {
            let input: RepoStatusInput = serde_json::from_value(arguments)
                .context("failed to parse richter_repo_status arguments")?;
            richter_repo_status(ctx, input).await
        }
        "richter_run_or_join" => {
            let input: RunOrJoinInput = serde_json::from_value(arguments)
                .context("failed to parse richter_run_or_join arguments")?;
            richter_run_or_join(ctx, input).await
        }
        "richter_active_runs" => {
            let input: ActiveRunsInput = serde_json::from_value(arguments)
                .context("failed to parse richter_active_runs arguments")?;
            richter_active_runs(ctx, input).await
        }
        "richter_recent_important_events" => {
            let input: RecentEventsInput = serde_json::from_value(arguments)
                .context("failed to parse richter_recent_important_events arguments")?;
            richter_recent_important_events(ctx, input).await
        }
        "richter_claim_paths" => {
            let input: ClaimPathsInput = serde_json::from_value(arguments)
                .context("failed to parse richter_claim_paths arguments")?;
            richter_claim_paths(ctx, input).await
        }
        "richter_release_paths" => {
            let input: ReleasePathsInput = serde_json::from_value(arguments)
                .context("failed to parse richter_release_paths arguments")?;
            richter_release_paths(ctx, input).await
        }
        "richter_explain_decision" => {
            let input: ExplainDecisionInput = serde_json::from_value(arguments)
                .context("failed to parse richter_explain_decision arguments")?;
            richter_explain_decision(ctx, input).await
        }
        "richter_get_run_summary" => {
            let input: GetRunSummaryInput = serde_json::from_value(arguments)
                .context("failed to parse richter_get_run_summary arguments")?;
            richter_get_run_summary(ctx, input).await
        }
        unknown => {
            warn!(tool = %unknown, "unknown tool requested");
            Ok(serde_json::json!({
                "error": "unknown_tool",
                "message": format!("Tool '{}' is not registered.", unknown)
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn daemon_offline_response(tool_name: &str) -> JsonValue {
    serde_json::json!({
        "error": "daemon_not_running",
        "message": format!(
            "The Richter daemon is not currently running. Start it with 'richter daemon start' or launch the Richter app."
        ),
        "tool": tool_name,
    })
}

fn daemon_offline_with_empty(field: &str) -> JsonValue {
    serde_json::json!({
        "error": "daemon_not_running",
        "message": "The Richter daemon is not currently running.",
        field: []
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> ToolContext {
        ToolContext {
            daemon_api_url: None,
            daemon_available: false,
            version: "0.1.0-test".to_string(),
            #[cfg(unix)]
            daemon_client: None,
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error() {
        let ctx = test_context();
        let result = dispatch_tool("nonexistent_tool", &ctx, serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(result["error"], "unknown_tool");
    }

    #[tokio::test]
    async fn richter_status_daemon_offline_returns_error() {
        let ctx = test_context();
        let result = richter_status(&ctx, StatusInput { repo_id: None })
            .await
            .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_repo_status_daemon_offline() {
        let ctx = test_context();
        let result = richter_repo_status(
            &ctx,
            RepoStatusInput {
                repo_id: "test-repo".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_run_or_join_daemon_offline_returns_fallback() {
        let ctx = test_context();
        let result = richter_run_or_join(
            &ctx,
            RunOrJoinInput {
                command: vec!["cargo".into(), "test".into()],
                cwd: None,
                label: None,
                wait: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
        assert_eq!(result["fallback"], "run_directly");
    }

    #[tokio::test]
    async fn richter_active_runs_daemon_offline() {
        let ctx = test_context();
        let result = richter_active_runs(
            &ctx,
            ActiveRunsInput {
                repo_id: None,
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_recent_important_events_daemon_offline() {
        let ctx = test_context();
        let result = richter_recent_important_events(
            &ctx,
            RecentEventsInput {
                limit: 10,
                repo_id: None,
                min_importance: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_claim_paths_daemon_offline() {
        let ctx = test_context();
        let result = richter_claim_paths(
            &ctx,
            ClaimPathsInput {
                paths: vec!["src/main.rs".to_string()],
                ttl: Some("30m".to_string()),
                agent_id: "test-agent".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_release_paths_daemon_offline() {
        let ctx = test_context();
        let result = richter_release_paths(
            &ctx,
            ReleasePathsInput {
                paths: vec!["src/main.rs".to_string()],
                agent_id: "test-agent".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_explain_decision_daemon_offline() {
        let ctx = test_context();
        let result = richter_explain_decision(
            &ctx,
            ExplainDecisionInput {
                decision_id: "dec-123".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn richter_get_run_summary_daemon_offline() {
        let ctx = test_context();
        let result = richter_get_run_summary(
            &ctx,
            GetRunSummaryInput {
                run_id: "run-456".to_string(),
            },
        )
        .await
        .unwrap();
        assert_eq!(result["error"], "daemon_not_running");
    }

    #[tokio::test]
    async fn dispatch_rejects_missing_required_field() {
        let ctx = test_context();
        // Missing required "repo_id" field should return an Err.
        let result = dispatch_tool("richter_repo_status", &ctx, serde_json::json!({})).await;
        assert!(
            result.is_err(),
            "dispatch should return Err for missing required field"
        );
    }

    #[test]
    fn daemon_offline_response_structure() {
        let resp = daemon_offline_response("test_tool");
        assert_eq!(resp["error"], "daemon_not_running");
        assert_eq!(resp["tool"], "test_tool");
    }

    #[test]
    fn daemon_offline_with_empty_structure() {
        let resp = daemon_offline_with_empty("items");
        assert_eq!(resp["error"], "daemon_not_running");
        assert!(resp["items"].is_array());
    }
}
