# Richter Privacy Documentation

## No Telemetry by Default

Richter collects **zero telemetry** by default. There is:

- No analytics SDK.
- No crash reporter that sends data off-device.
- No usage statistics collection.
- No update checker that phones home.
- No "share usage data" prompt.
- No network requests of any kind unless a model provider is explicitly
  configured.

The daemon makes outbound HTTPS connections **only** when the user has
configured an optional model provider (see `docs/MODELS.md`). Even then,
Richter sends only redacted, bounded payloads containing command output
summaries — never telemetry or usage data about the user's system.

## What Data Stays Local

All of the following data is stored **exclusively on the user's Mac**:

| Data | Storage Location | Format |
|---|---|---|
| Repository metadata (paths, Git state) | `~/.richter/db.sqlite` | SQLite |
| Command invocation history | `~/.richter/db.sqlite` | SQLite |
| Run records (exit codes, timestamps) | `~/.richter/db.sqlite` | SQLite |
| Run subscriber records | `~/.richter/db.sqlite` | SQLite |
| Event log | `~/.richter/db.sqlite` | SQLite |
| Importance classifications | `~/.richter/db.sqlite` | SQLite |
| Decision records (run/join/cache) | `~/.richter/db.sqlite` | SQLite |
| Path lease records | `~/.richter/db.sqlite` | SQLite |
| Model call audit trail | `~/.richter/db.sqlite` | SQLite |
| Raw command output (compressed) | `~/.richter/logs/` | gzip-compressed text |
| Daemon logs | `~/.richter/logs/daemon.log` | JSONL |
| MCP server logs | `~/.richter/logs/mcp.log` | JSONL |
| User configuration | `~/.richter/config.toml` | TOML |
| Per-repo configuration | `.richter/config.toml` (in repo) | TOML |
| Plugin manifests | `~/.richter/plugins/` | JSON |
| Daemon auth token | `~/.richter/daemon.token` | Random hex |
| Provider API keys | macOS Keychain | Encrypted |
| Model payload debug logs | `~/.richter/logs/model_payloads/` (if enabled) | text (redacted) |
| Shim wrappers | `~/.richter/shims/` | Shell scripts |
| Daemon binary | `~/.richter/bin/richterd` | Mach-O executable |

None of this data is sent to Richter's developers or to any third party.

## What Data Could Go to Model Providers

**Only if explicitly configured**, Richter sends data to user-specified model
providers. This data consists of:

1. **Cheap model calls (Tier 2):** Redacted, truncated (4KB max) command
   output for summarization and importance classification.

2. **Frontier model calls (Tier 3):** Redacted summaries of complex
   situations (test coverage analysis, multi-agent conflict summaries,
   queue prioritization) for ambiguous decision support.

### What Is NOT Sent to Model Providers (Even When Configured)

- Full command output logs
- Repository metadata (paths, Git state)
- File contents from the user's repositories
- Agent identification or process information
- User configuration
- Shell history
- Any file system data other than command output text
- The user's identity or machine identifiers
- Network information
- Any data from directories not explicitly configured as watched workspaces

### Provider Data Handling

Different model providers have different data handling policies:

| Provider | Data Retention | Training on API Data |
|---|---|---|
| **OpenAI** | 30 days (API) | No (API data not used for training) |
| **Anthropic** | 30 days | No (API data not used for training) |
| **DeepSeek** | Check DeepSeek's current policy | Check DeepSeek's current policy |
| **Ollama** (local) | None (runs locally) | N/A |
| **MLX** (local) | None (runs locally) | N/A |
| **llama.cpp** (local) | None (runs locally) | N/A |

Users concerned about data handling by cloud providers should use local
models (Ollama, MLX, llama.cpp) for the model pipeline. With local models,
**no data leaves the machine** at any point.

## Log Retention and Cleanup

### Default Retention

| Data | Default Retention | Configurable |
|---|---|---|
| Run output logs (compressed) | 7 days | `retention.runs_days` |
| Event records (in SQLite) | 30 days | `retention.events_days` |
| Model call audit trail | 90 days | `retention.model_calls_days` |
| Cache entries | Per-entry TTL | Per-command-class TTL |
| Daemon logs | 7 days (rotated daily) | Not configurable (keeps last 7 files) |
| MCP logs | 7 days (rotated daily) | Not configurable (keeps last 7 files) |
| Model payload debug logs | 1 day (if enabled) | Not configurable |

### Cleanup

Richter runs periodic cleanup:

- **Every hour**: Prune expired cache entries and expired path leases.
- **Every day (at daemon startup and every 24h)**: Prune events, runs, and
  logs exceeding retention limits.
- **On demand**: `richter cleanup --all`

### Manual Deletion

To remove all Richter data:

```bash
richter uninstall --all        # Remove daemon, shims, shell integration
rm -rf ~/.richter              # Remove all stored data
rm -f ~/Library/LaunchAgents/com.richter.daemon.plist
```

To remove only logs:

```bash
richter cleanup --logs
```

To remove only the database (resets all history):

```bash
richter cleanup --database
# Warning: this deletes all event, run, and agent history
```

## Redaction Guarantees

Richter's redaction engine provides **best-effort** secret removal. See
`docs/SECURITY.md` for the complete list of redacted patterns.

Key guarantees:

1. **Redaction happens before storage.** Secrets are stripped from output
   before it is written to compressed log files or the database.

2. **Redaction happens before model calls.** Text sent to model providers
   is redacted independently of the stored version (belt-and-suspenders).

3. **Redacted values are not logged.** The redaction engine processes text
   in a single pass and does not store the original values.

4. **Known patterns are covered.** The redaction engine covers all major
   API key formats, token formats, and credential patterns.

What Redaction Does NOT Guarantee:

- **Novel or obfuscated secrets.** If a secret doesn't match known patterns,
  it may not be caught.
- **Secrets in binary data.** Redaction operates on text output. Binary data
  in command output is not processed.
- **Secrets intentionally echoed.** If a user runs `echo $MY_SECRET`, the
  value may appear in command output and in the agent's terminal. Richter
  captures what the command prints.

## Data Storage Locations

```
~/.richter/
├── bin/
│   └── richterd              # Daemon binary
├── shims/                    # Shell shim scripts
│   ├── cargo
│   ├── npm
│   ├── pnpm
│   └── ... (27 total)
├── plugins/                  # Agent plugin manifests
│   ├── claude.json
│   ├── codex.json
│   └── ...
├── config.toml               # Global configuration
├── db.sqlite                  # SQLite database (WAL mode)
├── db.sqlite-wal              # WAL journal
├── db.sqlite-shm              # WAL shared memory
├── daemon.sock                # Unix domain socket (0600)
├── daemon.token               # Auth token (0600)
└── logs/
    ├── daemon.log             # Current daemon log
    ├── daemon.log.1           # Rotated
    ├── mcp.log                # Current MCP log
    ├── runs/                  # Compressed run output
    │   ├── <run_id>.log.gz
    │   └── ...
    ├── cache/                 # Cached run metadata
    │   └── ...
    └── model_payloads/        # Debug payloads (if enabled)
        └── ...
```

All files under `~/.richter/` are owned by the current user with restrictive
permissions (directories `0700`, files `0600` where appropriate).

## User Controls

Richter provides the following privacy controls:

### Retention

```toml
# ~/.richter/config.toml
[retention]
runs_days = 7          # How long to keep run output logs
events_days = 30       # How long to keep event records
model_calls_days = 90  # How long to keep model call audit trail
```

### Redaction

```toml
[redaction]
enabled = true
# Additional patterns to redact (beyond built-in):
extra_patterns = [
    "my-company-internal-*",
    "ACME_SECRET_.*"
]
```

### Model Payload Logging

```toml
[models.debug]
log_payloads = false    # Disabled by default
log_responses = false
```

### Disabling Models

```bash
richter model disable     # No model calls of any kind
richter model enable      # Re-enable
```

Or remove the `[models]` section from config entirely.

### Immediate Cleanup

```bash
richter cleanup --all     # Purge all logs, cache, and old events
richter cleanup --runs    # Purge run output logs only
richter cleanup --cache   # Clear the result cache
```

### Data Export

Export all Richter data (for backup or migration):

```bash
richter export --output richter-backup.tar.gz
```

This creates a tarball containing:

- The SQLite database (with redaction already applied — no raw secrets in DB).
- Config files (redacted).
- Plugin manifests.
- Does **not** include Keychain items or the auth token.

### Complete Removal

```bash
richter uninstall --all
rm -rf ~/.richter
```

This removes all Richter data, configurations, shims, and logs from the
machine. No data remains.
