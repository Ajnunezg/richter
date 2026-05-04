# Richter Troubleshooting Guide

## Diagnostic Command

Always start with:

```bash
richter doctor
```

This runs a comprehensive health check and reports the status of every
component. Most issues are surfaced here.

For more detail:

```bash
richter doctor --verbose
```

## Daemon Not Starting

### Symptoms

- `richter status` reports "daemon not running."
- CLI commands fail with "could not connect to daemon."
- The menu bar icon shows "disconnected."

### Checks

**1. Verify the daemon binary exists and is executable:**

```bash
ls -la ~/.richter/bin/richterd
# Should show: -rwxr-xr-x ... richterd
```

If missing, reinstall:

```bash
richter install daemon
```

**2. Check the LaunchAgent plist:**

```bash
cat ~/Library/LaunchAgents/com.richter.daemon.plist
```

Expected content (approximate):
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" ...>
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.richter.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/YOU/.richter/bin/richterd</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
```

If the plist is missing or malformed:

```bash
richter uninstall daemon
richter install daemon
```

**3. Try starting the daemon directly to see errors:**

```bash
~/.richter/bin/richterd
```

Look for error messages about:
- Missing directory permissions (`~/.richter/` should be `0700`)
- Database migration failures
- Port/socket binding errors

**4. Check permissions:**

```bash
ls -la ~/.richter/
# Directory should be: drwx------
# daemon.sock should be: srwx------
# daemon.token should be: -rw-------
```

Fix permissions:

```bash
chmod 0700 ~/.richter
chmod 0600 ~/.richter/daemon.token 2>/dev/null
```

**5. Check for port conflicts:**

If another process is already using the socket path:

```bash
lsof ~/.richter/daemon.sock
```

If a stale socket exists, remove it and restart:

```bash
rm ~/.richter/daemon.sock
richter daemon restart
```

**6. Check system logs:**

```bash
log show --predicate 'subsystem == "com.richter.daemon"' --last 10m
```

### Common Causes

- **macOS permissions**: The first time you run the daemon, macOS may show a
  "richterd wants to access files" prompt. Approve it.
- **Stale lock files**: If the daemon crashed, `daemon.sock` might be stale.
  Delete it and restart.
- **Database corruption**: If `db.sqlite` is corrupted, rename it and restart.
  Richter will create a fresh database, but history will be lost.
  ```bash
  mv ~/.richter/db.sqlite ~/.richter/db.sqlite.backup
  richter daemon restart
  ```

## Shims Not Working

### Symptoms

- Commands execute directly without going through Richter.
- `which cargo` shows the system path, not `~/.richter/shims/cargo`.
- No "joined existing run" or "cached hit" messages appear.

### Checks

**1. Verify shim installation:**

```bash
richter shims list
```

Should show a table of installed shims.

**2. Check PATH ordering:**

```bash
echo $PATH | tr ':' '\n' | head -5
```

`~/.richter/shims` should appear **before** `/usr/local/bin`, `/opt/homebrew/bin`,
`~/.cargo/bin`, etc.

If not:

```bash
# Check your shell rc file
grep -n "richter" ~/.zshrc ~/.bashrc ~/.bash_profile 2>/dev/null
```

Ensure the line `export PATH="$HOME/.richter/shims:$PATH"` (or similar) appears
and isn't overridden later in the file.

**3. Verify a shim script:**

```bash
cat ~/.richter/shims/cargo
```

Should contain:
```bash
#!/bin/bash
exec richter run --shim-name cargo -- "$@"
```

**4. Test a shim directly:**

```bash
~/.richter/shims/cargo --version
```

Should execute normally. If it hangs or errors, the daemon might not be
running (see "Daemon Not Starting" above).

**5. Check shell integration:**

```bash
richter doctor --shell
```

### Common Causes

- **Shell rc file order**: A package manager (Homebrew, nvm, rustup) appends
  to PATH after the Richter line. Move the Richter PATH line to the **end**
  of your rc file (PATH is searched left to right, so earlier entries win).
  ```bash
  # In ~/.zshrc, put this LAST:
  export PATH="$HOME/.richter/shims:$PATH"
  ```

- **Shell not reloaded**: Run `source ~/.zshrc` or open a new terminal.

## PATH Issues

### Tools Bypassing Richter

Some tools resolve their own PATH or have hardcoded paths:

- **IDE-integrated terminals**: Some IDEs set their own PATH. Check IDE
  terminal settings.
- **nvm/rbenv/pyenv**: These tools manage their own PATH. Ensure
  `~/.richter/shims` is injected by your shell rc file after these tools
  initialize.
- **Docker containers**: Richter shims only work on the host. Containers
  have their own PATH.

### Fix

For interactive shells, ensure your rc file has the Richter PATH line at
the **very end**:

```bash
# .zshrc — end of file
export PATH="$HOME/.richter/shims:$PATH"
```

For non-interactive shells (used by some agents), add the PATH line to
`~/.zshenv` or `~/.bashrc`.

## Permission Errors

### "Permission denied" on socket

```bash
ls -la ~/.richter/daemon.sock
```

Should show: `srw------- 1 YOU staff ...`

If permissions are wrong:

```bash
chmod 0600 ~/.richter/daemon.sock
```

### "Permission denied" on config or database

```bash
ls -la ~/.richter/
```

Everything under `~/.richter/` should be owned by you. If not:

```bash
sudo chown -R $(whoami) ~/.richter
```

### macOS "App wants to access files" prompt

The SwiftUI app or daemon may trigger macOS file access prompts. Approve
them. If you denied them previously, reset in System Settings → Privacy &
Security → Files and Folders, then restart the app.

## MCP Connection Failures

### Symptoms

- AI agent says "MCP server disconnected" or "MCP tool not available."
- `richter doctor --mcp` shows failures.

### Checks

**1. Verify the MCP binary:**

```bash
which richter-mcp
richter-mcp --version
```

If not found, rebuild or reinstall:

```bash
richter install mcp --agent <agent-name>
```

**2. Test stdio MCP manually:**

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}' | richter-mcp stdio
```

Should return a JSON-RPC response. If it errors, check:
- `~/.richter/logs/mcp.log` for MCP-specific errors.
- Daemon is running (`richter status`).

**3. Check agent MCP configuration:**

For Claude Code:
```bash
cat ~/.claude/claude_desktop_config.json
# or
cat .mcp.json
```

Should contain a `richter` entry with the correct `command` and `args`.

For Codex:
```bash
cat ~/.codex/mcp.json
```

**4. Check MCP transport:**

Stdio MCP uses stdin/stdout. Ensure the agent is spawning `richter-mcp stdio`
correctly. Common issues:
- Agent expects a specific working directory for MCP binaries.
- Agent's PATH doesn't include `richter-mcp`. Use an absolute path:
  ```json
  "command": "/Users/YOU/.cargo/bin/richter-mcp"
  ```

**5. MCP logs:**

```bash
tail -f ~/.richter/logs/mcp.log
```

Look for JSON-RPC protocol errors, connection drops, or tool registration
failures.

## Agent Detection Issues

### Symptoms

- Dashboard "Agents" page is empty or missing agents.
- `richter agents` shows fewer agents than expected.

### Checks

**1. Agent must use shims or MCP:**

Richter detects agents by:
- Processes that invoke Richter shims.
- Processes that connect to the MCP server.
- Parent process command-line pattern matching.

If an agent uses neither shims nor MCP (e.g., it runs commands through a
container or a custom execution backend), Richter won't detect it.

**2. Check process detection interval:**

Agents are discovered periodically (default every 30 seconds). New agents
may take up to 30 seconds to appear. Run a shim command to trigger immediate
detection.

**3. Check for custom agent names:**

If the agent uses a non-standard process name, add a plugin manifest:

```json
// ~/.richter/plugins/my-custom-agent.json
{
  "name": "my-custom-agent",
  "agent_detection": {
    "process_names": ["my-agent-binary", "my-agent-helper"],
    "command_patterns": ["^my-agent", "^ma run"]
  }
}
```

Then restart the daemon:

```bash
richter daemon restart
```

**4. Session tracking:**

Richter tracks agent sessions. If an agent's MCP connection drops, the agent
may appear "stale." It reappears when the agent reconnects or runs a shim
command.

## Cache Invalidation Issues

### Symptoms

- A cached result is returned when the code has changed.
- Tests pass from cache but should have failed with new code.

### How Cache Works

Richter's cache is **fingerprint-based**, not time-based. A cached result is
only returned if the fingerprint matches exactly. The fingerprint includes
HEAD SHA, dirty tree hash, lockfiles, toolchain versions, and env vars.

### Checks

**1. Verify the fingerprint includes the changed file:**

Changes to source files that aren't tracked by Git (untracked, not in the
dirty tree) may not change the fingerprint. Check:

```bash
git status
```

If the changed file appears as untracked and isn't in a relevant directory
for the command class, it may not affect the fingerprint. This is by design
— not all untracked files affect build/test results. If you want more
conservative caching, add the directory to the per-repo config:

```toml
[commands.test]
extra_fingerprint_paths = ["src/", "tests/"]
```

**2. Bypass cache for a single run:**

```bash
richter run --no-cache -- cargo test
```

**3. Clear the entire cache:**

```bash
richter cleanup --cache
```

**4. Check cache TTL:**

If the TTL is very short, you might never see cache hits. Check:

```bash
cat .richter/config.toml  # per-repo
cat ~/.richter/config.toml  # global
```

Look for `[commands.*]` sections with `cache_ttl`.

### Common Causes

- **Lockfile not changed**: If `Cargo.lock` or `pnpm-lock.yaml` is in
  `.gitignore` and not committed, it may not be part of the fingerprint.
- **Environment variables**: Test behavior depends on `RUST_LOG`, `NODE_ENV`,
  etc. If these change between runs, the fingerprint changes and cache won't
  hit. Ensure env vars are consistent or add them to `extra_env_vars` in
  config.

## Resource Deadlocks

### Symptoms

- Commands are permanently queued ("queued for X minutes" forever).
- Heavy build never starts despite no other active builds.
- "Resource class at capacity" but no runs are visible.

### Checks

**1. Check active runs:**

```bash
richter runs --status active
```

Look for runs that are stuck (long duration, no output).

**2. Check for orphaned processes:**

```bash
richter doctor --processes
```

This lists processes Richter is tracking. Orphaned processes (child processes
whose parent Richter didn't track) may hold resource locks.

**3. Check resource limits:**

```bash
richter status
```

Shows current resource utilization and limits. If limits are too low for your
workload, increase them:

```toml
[resources]
max_heavy_runs_per_repo = 2  # was 1
max_heavy_runs_global = 4    # was 3
```

**4. Kill stuck runs:**

```bash
richter run kill <run-id>
```

Or for all runs in a repo:

```bash
richter run kill --repo <repo-name> --all
```

**5. Pause/resume coordination:**

```bash
richter pause      # Pause all new run starts
richter resume     # Resume
```

### Common Causes

- A build process is waiting for user input (TTY). Richter should detect this
  and kill after a timeout, but edge cases exist.
- A process has been SIGSTOP'd (suspended via Ctrl-Z). Richter can't detect
  this from process state alone.
- A resource lock wasn't released after a crash. Restart the daemon to
  clear all locks:
  ```bash
  richter daemon restart
  ```

## Debug Logging

### Enable verbose logs

```bash
# Set daemon log level
richter settings set log.level debug

# Or in config:
# ~/.richter/config.toml
[log]
level = "debug"
```

Restart the daemon for changes to take effect:

```bash
richter daemon restart
```

### View logs

```bash
# Daemon logs (JSONL)
tail -f ~/.richter/logs/daemon.log

# Parse with jq for readability
tail -100 ~/.richter/logs/daemon.log | jq '.message'

# MCP logs
tail -f ~/.richter/logs/mcp.log

# System-level logs (macOS unified log)
log stream --predicate 'subsystem == "com.richter.daemon"' --level debug
```

### Common log patterns

```
WARN  richter_daemon::scheduler  Resource class heavy-build at capacity (1/1), queuing run abc123
INFO  richter_daemon::run_mgr    Fingerprint match: joining run def456 as subscriber (2 total)
ERROR richter_daemon::classifier Unknown command class for: my-custom-tool --flag
INFO  richter_daemon::redact     Redacted 3 secrets from 12KB output
WARN  richter_daemon::model      Budget exceeded: 10/10 frontier calls today, skipping
```

## Quick Reference

| Command | Purpose |
|---|---|
| `richter doctor` | Full system health check |
| `richter doctor --verbose` | Detailed health check with diagnostics |
| `richter status` | Current daemon status |
| `richter daemon restart` | Restart the daemon |
| `richter daemon stop` | Stop the daemon |
| `richter cleanup --cache` | Clear result cache |
| `richter cleanup --logs` | Delete log files |
| `richter uninstall daemon && richter install daemon` | Full daemon reinstall |
| `richter settings set log.level debug` | Enable debug logging |
| `richter settings show` | Show all current settings |
| `richter shims list` | List installed shims |
| `richter runs --status active` | Show active runs |
| `richter run kill <run-id>` | Kill a stuck run |
| `richter pause` / `richter resume` | Pause/resume coordination |
