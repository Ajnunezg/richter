//! Tool registry: declares all Richter MCP tools and their JSON Schemas.
//!
//! The registry is the single source of truth for what tools the MCP server
//! advertises to clients during the `tools/list` request.

use serde_json::Value as JsonValue;

/// Represents a registered MCP tool with its JSON Schema input definition.
#[derive(Debug, Clone)]
pub struct RegisteredTool {
    /// The tool name (e.g., "richter_status").
    pub name: &'static str,
    /// Human-readable description for LLM consumption.
    pub description: &'static str,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: JsonValue,
}

/// Return all registered Richter MCP tools with their JSON schemas.
pub fn all_tools() -> Vec<RegisteredTool> {
    vec![
        RegisteredTool {
            name: "richter_status",
            description: "Get the global Richter system status: daemon health, active/queued runs, cache stats, and system pressure.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repo_id": {
                        "type": "string",
                        "description": "Optional repository id to filter status"
                    }
                },
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_repo_status",
            description: "Get detailed status for a specific repository: branch, dirty state, active agents, runs, and recent events.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repo_id": {
                        "type": "string",
                        "description": "The repository id to query"
                    }
                },
                "required": ["repo_id"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_run_or_join",
            description: "Submit a shell command through Richter. Richter will decide whether to run it, join an existing equivalent run, return a cached result, or queue it based on resource availability and fingerprint matching.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The command to execute as an argv array"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Working directory for the command"
                    },
                    "label": {
                        "type": "string",
                        "description": "Optional human-readable label for the run"
                    },
                    "wait": {
                        "type": "boolean",
                        "description": "Whether to wait for completion before returning (default: true)"
                    }
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_active_runs",
            description: "List currently active (running and queued) runs, optionally filtered by repository.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repo_id": {
                        "type": "string",
                        "description": "Optional repository id filter"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of runs to return (default: 50)",
                        "default": 50
                    }
                },
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_recent_important_events",
            description: "Get recent important events filtered through Richter's importance pipeline. These are the highest-signal events agents should be aware of.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of events (default: 50)",
                        "default": 50
                    },
                    "repo_id": {
                        "type": "string",
                        "description": "Optional repository id filter"
                    },
                    "min_importance": {
                        "type": "integer",
                        "description": "Minimum importance threshold 0-100 (default: 0)",
                        "minimum": 0,
                        "maximum": 100
                    }
                },
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_claim_paths",
            description: "Claim files or directories for exclusive/advisory access. Other agents will be notified of conflicts.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File or directory paths to claim"
                    },
                    "ttl": {
                        "type": "string",
                        "description": "Time-to-live for the claim (e.g., '30m', '1h')"
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "Identifier of the claiming agent"
                    }
                },
                "required": ["paths", "agent_id"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_release_paths",
            description: "Release previously claimed file or directory paths, making them available to other agents.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "File or directory paths to release"
                    },
                    "agent_id": {
                        "type": "string",
                        "description": "Identifier of the releasing agent"
                    }
                },
                "required": ["paths", "agent_id"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_explain_decision",
            description: "Explain a Richter scheduling or deduplication decision, including the matched fingerprint and confidence.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "decision_id": {
                        "type": "string",
                        "description": "The decision id to explain"
                    }
                },
                "required": ["decision_id"],
                "additionalProperties": false
            }),
        },
        RegisteredTool {
            name: "richter_get_run_summary",
            description: "Get a detailed summary of a specific run: command, timeline, exit code, subscribers, cache status, and log paths.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "The run id to summarize"
                    }
                },
                "required": ["run_id"],
                "additionalProperties": false
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tools_returns_nine_tools() {
        let tools = all_tools();
        assert_eq!(tools.len(), 9);
    }

    #[test]
    fn all_tools_have_name_and_description() {
        for tool in all_tools() {
            assert!(!tool.name.is_empty(), "tool name must not be empty");
            assert!(
                !tool.description.is_empty(),
                "tool {} has empty description",
                tool.name
            );
            assert!(
                tool.input_schema.is_object(),
                "tool {} input_schema is not an object",
                tool.name
            );
        }
    }

    #[test]
    fn all_tool_names_are_unique() {
        let mut names: Vec<&str> = all_tools().iter().map(|t| t.name).collect();
        let len_before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), len_before, "tool names must be unique");
    }
}
