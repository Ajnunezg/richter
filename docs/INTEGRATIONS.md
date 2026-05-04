# Richter Integration Guide

Richter integrates with AI coding agents through three mechanisms: shell shims,
MCP servers, and agent-specific hooks. This guide covers setup for each.

## Integration Mechanisms

```
Agent Type              Primary Mechanism       Secondary
─────────────────────────────────────────────────────────
Claude Code             MCP + hooks             Shell shims
Codex CLI               MCP + hooks             Shell shims
Droid                   MCP (if available)      Shell shims
Forge Code              MCP (if available)      Shell shims
Kimi                    MCP (if available)      Shell shims
MiniMax                 MCP (if available)      Shell shims
Generic CLI agent       Shell shims             N/A
Generic MCP agent       MCP                     N/A
```

Shell shims are the baseline integration mechanism — they work for any agent
that executes shell commands. MCP provides richer query capabilities. Hooks
add agent-local status awareness.

## MCP Server Setup

Richter's MCP server exposes tools and resources so agents can query Richter
state and make decisions without shell interception.

### MCP Tools

| Tool | Description |
|---|---|
| `richter_status` | Global Richter status (daemon uptime, active runs, queued runs, system pressure) |
| `richter_repo_status` | Status for a specific repository |
| `richter_run_or_join` | Submit a command for execution with run-or-join semantics |
| `richter_active_runs` | List currently active runs |
| `richter_recent_important_events` | List recent important events (filterable by repo) |
| `richter_claim_paths` | Claim advisory leases on files/directories |
| `richter_release_paths` | Release previously claimed leases |
| `richter_explain_decision` | Explain why Richter made a particular run-or-join decision |
| `richter_get_run_summary` | Get the summary for a completed run |

### MCP Resources

| URI | Description |
|---|---|
| `richter://global/status` | Current global status (JSON) |
| `richter://repo/{repo_id}/status` | Status for a repo (active runs, agents, events) |
| `richter://run/{run_id}/summary` | Summary of a specific run (exit code, output, subscribers) |
| `richter://agent/{agent_id}/inbox` | Agent inbox (important events relevant to this agent) |

### Stdio MCP Transport

For agents that spawn MCP servers as subprocesses:

```json
{
  "mcpServers": {
    "richter": {
      "command": "richter-mcp",
      "args": ["stdio"]
    }
  }
}
```

The `richter-mcp stdio` process communicates via stdin/stdout JSON-RPC. It
writes all diagnostic logs to a file (`~/.richter/logs/mcp.log`), never to
stdout, preserving the JSON-RPC channel.

### HTTP/SSE MCP Transport

For agents that connect to MCP over HTTP:

```json
{
  "mcpServers": {
    "richter": {
      "url": "http://localhost:0/api/mcp",
      "transport": "sse"
    }
  }
}
```

The actual URL uses the Unix domain socket path plus the auth token. The daemon
handles SSE streaming on `/api/mcp/sse` and message posting on `/api/mcp/message`.

### Installing MCP for Specific Agents

#### Claude Code

```bash
richter install mcp --agent claude
```

This creates or updates the appropriate Claude MCP configuration file.
Claude Code looks for MCP configuration in:
- `~/.claude/claude_desktop_config.json` (global)
- `.mcp.json` in the project root (project-local)

The installer adds a `richter` entry to whichever file already exists (or
creates the project-local one if neither exists).

Verify:

```bash
richter doctor --mcp
```

#### Codex (Codex CLI)

```bash
richter install mcp --agent codex
```

Codex CLI MCP configuration lives in `~/.codex/mcp.json` or the project-local
`.codex/mcp.json`. The installer detects the active configuration file.

#### Generic Agents

For any agent that supports MCP with a stdio transport:

```bash
richter install mcp --agent generic --output ~/.config/mcp/richter.json
```

This generates a standalone MCP config snippet. Point your agent's MCP
configuration at the generated file or at the `richter-mcp stdio` binary.

## Shell Shim Integration

Shell shims work for **any agent** that runs shell commands. They don't require
agent-specific configuration.

### How Shims Work

```
Agent runs: npm test -- --grep auth
     │
     ▼
Shell resolves npm to ~/.richter/shims/npm (first in PATH)
     │
     ▼
Shim rewrites to: richter run --shim-name npm -- test -- --grep auth
     │
     ▼
richter CLI sends RunRequest to daemon via Unix socket
     │
     ▼
Daemon classifies, fingerprints, and runs-or-joins
     │
     ▼
Output streams back to agent's terminal
```

### Installing Shims

```bash
richter install shims
```

This creates wrapper scripts in `~/.richter/shims/` for all supported tools.

### Default Shim Set

| Ecosystem | Tools |
|---|---|
| JavaScript/TypeScript | `npm`, `pnpm`, `yarn`, `bun`, `node`, `npx`, `turbo`, `nx`, `deno`, `tsc`, `eslint`, `jest`, `vitest`, `playwright` |
| Rust | `cargo` |
| Python | `python`, `pytest`, `uv`, `ruff` |
| Go | `go` |
| Swift/Xcode | `swift`, `xcodebuild` |
| Java/Kotlin | `gradle`, `mvn` |
| Build systems | `make`, `cmake`, `ninja`, `bazel` |

### How Shims Preserve Normal Behavior

Shims are transparent wrappers. They:

- Pass through `--help`, `--version`, and other non-execution flags directly to
  the real tool without involving Richter.
- Detect interactive/TTY mode and pass through without interception.
- Preserve stdin, stdout, stderr, and exit codes.
- Forward signals (SIGINT, SIGTERM) correctly.
- Handle unknown subcommands by passing through to the real tool.

### Verifying Shims

```bash
which npm                  # ~/.richter/shims/npm
npm --version              # works normally
richter shims list         # list all installed shims
richter doctor --shims     # validate shim installation
```

## Claude Code Hooks

Beyond MCP, Claude Code supports hooks that Richter can install for deeper
integration.

### Installing Claude Code Hooks

```bash
richter install hooks --agent claude
```

This generates and installs hook configuration snippets:

### PreToolUse Hook

Before Claude Code executes a Bash tool command, Richter can:

- Check if an equivalent command is already running.
- Warn if the command conflicts with another agent's path lease.
- Suggest joining an existing run instead of starting a duplicate.

Configuration placed in `~/.claude/settings.json` or project-local `.claude/settings.json`.

### PostToolUse Hook

After a command executes, Richter can:

- Report if the command was joined to an existing run.
- Surface cache status.
- Push important test failures to the agent.

### Statusline Integration

A statusline script shows Claude Code's context in the terminal:

```bash
# In your shell prompt (richter install shell adds this automatically)
richter statusline --repo --branch --active-runs
```

Output format:
```
[my-project:main] ⚡2 runs · ⏳1 queued · 💾3 cached
```

The components:
- `[repo:branch]` — current repo and branch
- `⚡N runs` — active runs in the current repo
- `⏳N queued` — queued commands waiting for resources
- `💾N cached` — cache hits for this session

### Notification Hook

Richter can push important events to Claude Code:

```
Richter: ⚠ pytest failed 3 tests in repo my-project [View]
```

### Stop Hook

When Claude Code stops, Richter can release any path leases held by that agent.

## Codex Hooks

### Installing Codex Hooks

```bash
richter install hooks --agent codex
```

Codex supports hooks for tool execution and session lifecycle events.

### PreToolUse Hook

Before executing a shell command, the hook:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "command": "richter hook pre-tool-use --agent codex --command \"$COMMAND\" --cwd \"$CWD\"",
        "timeout": 5000
      }
    ]
  }
}
```

Richter responds with a JSON decision:
```json
{
  "action": "proceed|join|warn|block",
  "message": "Joined existing cargo test run #42 (2 subscribers).",
  "run_id": "abc123",
  "cache_hit": false
}
```

### PostToolUse Hook

After command execution:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Bash",
        "command": "richter hook post-tool-use --agent codex --run-id \"$RUN_ID\" --exit-code \"$EXIT_CODE\"",
        "timeout": 5000
      }
    ]
  }
}
```

### SessionStart/SessionStop Hooks

Track agent sessions for lease management and agent detection:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "command": "richter hook session-start --agent codex --cwd \"$CWD\"",
        "timeout": 3000
      }
    ],
    "SessionStop": [
      {
        "command": "richter hook session-stop --agent codex",
        "timeout": 3000
      }
    ]
  }
}
```

### MCP Configuration for Codex

Add to the agent's MCP configuration:

```json
{
  "mcpServers": {
    "richter": {
      "command": "richter-mcp",
      "args": ["stdio"]
    }
  }
}
```

## Adding Support for New Agents

Richter uses a plugin manifest system to support new agents without code changes.

### Plugin Manifest Format

Create a JSON file in `~/.richter/plugins/<agent-name>.json`:

```json
{
  "name": "my-agent",
  "display_name": "My Agent",
  "version": "1.0.0",
  "description": "My custom AI coding agent integration",
  "agent_detection": {
    "process_names": ["my-agent", "my-agent-helper"],
    "command_patterns": ["^my-agent", "^ma "],
    "env_vars": ["MY_AGENT_HOME"]
  },
  "mcp": {
    "transport": "stdio",
    "command": "richter-mcp",
    "args": ["stdio"],
    "config_paths": [
      "~/.my-agent/mcp.json",
      ".my-agent/mcp.json"
    ]
  },
  "hooks": {
    "supported": false,
    "note": "My Agent does not support hooks; use MCP for integration."
  },
  "shims": {
    "enabled": true,
    "note": "Shell shims work automatically for any agent."
  },
  "install": {
    "mcp_command": "echo 'Add this to your MCP config: richter-mcp stdio'",
    "verify_command": "richter doctor --agent my-agent"
  }
}
```

### Plugin Discovery

Richter auto-discovers plugins in `~/.richter/plugins/` at daemon startup.
Plugins can be enabled/disabled from Settings or via:

```bash
richter plugin enable my-agent
richter plugin disable my-agent
richter plugin list
```

### Detection

Richter detects agents by:

1. **Process name matching** — scanning the process tree for known agent binary
   names and matching against plugin `process_names`.
2. **Command-line pattern matching** — examining parent process command lines for
   patterns in `command_patterns`.
3. **Environment variable detection** — checking for agent-specific env vars from
   `env_vars`.
4. **Shim invocation** — any process that invokes a Richter shim is tracked as a
   potential agent session.

Detection is passive and does not require root permissions. It runs periodically
(by default every 30 seconds) and on shim invocations.

### Hook Generation

For agents that support hooks, the plugin manifest can specify hook types with
install/uninstall commands. Richter generates hook configuration snippets that
the agent's settings system understands.

### Contributing Agent Plugins

Built-in agent support lives in `integrations/<agent>/`. Community plugins can
be submitted as PRs adding an integration directory with:

- `plugin.json` — the plugin manifest
- `install.sh` — installation script (optional)
- `hooks/` — hook templates (optional)
- `README.md` — documentation

Richter ships with built-in plugins for Claude Code and Codex. The plugin system
ensures adding support for Droid, Forge Code, Kimi, MiniMax, and future agents
requires no code changes to the daemon or app — only a JSON manifest.
