# Richter Operational Runbook

## Quick Reference

| Item | Value |
|------|-------|
| Data directory | `~/.richter/` |
| Daemon socket | `~/.richter/daemon.sock` |
| Auth token | `~/.richter/auth_token` (0600) |
| PID file | `~/.richter/daemon.pid` |
| Database | `~/.richter/richter.db` |
| Encryption key | `~/.richter/db.key` (0600) |
| Logs | `~/.richter/logs/richter.*` (daily rotation) |
| Config | `~/.richter/config.toml` |
| Shims | `~/.richter/shims/` |

## Common Operations

### Start the Daemon

```bash
# Via CLI (prompts if not running)
richter status

# Manually
richter-daemon

# With debug logging
RUST_LOG=richter_daemon=debug richter-daemon
```

### Stop the Daemon

```bash
# Graceful (30s drain for active runs)
kill $(cat ~/.richter/daemon.pid)

# Force
kill -9 $(cat ~/.richter/daemon.pid)
```

### Check Daemon Health

```bash
richter doctor      # Full diagnostic
richter status      # Current state
```

## Troubleshooting

### "Another Richter daemon is already running"

**Cause:** A PID file exists and the process is still alive.

**Fix:**
```bash
# Check if the daemon is actually running
ps aux | grep richter-daemon

# If running, stop it gracefully
kill $(cat ~/.richter/daemon.pid)

# If NOT running (stale PID file), remove it
rm ~/.richter/daemon.pid
```

### "Daemon not responding" / Stale Socket

**Cause:** The daemon crashed or was killed without cleanup, leaving a stale socket file.

**Fix:**
```bash
# Remove stale socket
rm ~/.richter/daemon.sock

# Restart
richter-daemon
```

### "Auth token file not found"

**Cause:** Token is rotated on every daemon start. If the daemon is not running, the token doesn't exist.

**Fix:**
```bash
# Start the daemon — it generates a new token automatically
richter-daemon
```

### Permission Errors

**Symptoms:** `0600` permission errors on `daemon.sock`, `auth_token`, or `db.key`.

**Fix:**
```bash
# Richter auto-corrects permissions on startup, but manual fix:
chmod 600 ~/.richter/auth_token ~/.richter/db.key
chmod 700 ~/.richter/
```

### Hung / Unresponsive Daemon

**Symptoms:** CLI commands hang, no output from `richter status`.

**Diagnosis:**
```bash
# Check process is alive
ps aux | grep richter-daemon

# Check CPU/memory usage
top -pid $(cat ~/.richter/daemon.pid)

# Check recent logs
tail -100 ~/.richter/logs/richter.*

# Check active runs in database
sqlite3 ~/.richter/richter.db "SELECT id, command, status FROM runs WHERE status IN ('queued', 'running')"
```

**Fix:**
```bash
# Graceful stop with drain
kill $(cat ~/.richter/daemon.pid)
# Wait up to 30s for active runs to finish

# If still hung after 30s, force kill
kill -9 $(cat ~/.richter/daemon.pid)
rm -f ~/.richter/daemon.sock ~/.richter/daemon.pid

# Orphaned runs will be reconciled on next startup
richter-daemon
```

### Database Corruption

**Symptoms:** `Database integrity check failed` in logs.

**Fix:**
```bash
# Check integrity
sqlite3 ~/.richter/richter.db "PRAGMA integrity_check"

# If corrupted, restore from backup
cp ~/.richter/richter.db.backup ~/.richter/richter.db

# If no backup, recreate (you WILL lose history)
rm ~/.richter/richter.db
richter-daemon  # Creates fresh database
```

### Reset Everything

**Warning:** This deletes all Richter state, history, and configuration.

```bash
# Stop the daemon
kill $(cat ~/.richter/daemon.pid) 2>/dev/null

# Remove all state
rm -rf ~/.richter/

# Restart fresh
richter-daemon
```

## Monitoring

### Key Metrics (Prometheus format)

Available via `GET /metrics/prometheus` on the Unix socket:

| Metric | Meaning |
|--------|---------|
| `richter_runs_started` | Total new process spawns |
| `richter_runs_completed` | Total completed runs |
| `richter_cache_hits` | Results served from cache |
| `richter_duplicates_prevented` | Joined existing runs |
| `richter_auth_failures` | Invalid token attempts |
| `richter_rate_limited` | Rejected by rate limit |
| `richter_runs_rejected` | Rejected (destructive, etc.) |

### Log Format

Default: human-readable. JSON format via `RUST_LOG_FORMAT=json`:

```bash
RUST_LOG_FORMAT=json richter-daemon
```

### Health Endpoint

```bash
# Via curl through Unix socket
curl --unix-socket ~/.richter/daemon.sock \
  -H "Authorization: Bearer $(cat ~/.richter/auth_token)" \
  http://localhost/health
```

## Orphaned Run Recovery

When the daemon starts, it automatically:
1. Detects runs left in `queued` or `running` status from a previous instance
2. Marks them as `Failed` with exit code -1
3. Logs the reconciliation count

No manual intervention required.

## Cache Management

```bash
# Check cache size
sqlite3 ~/.richter/richter.db "SELECT COUNT(*) FROM run_cache"

# Force eviction of expired entries
sqlite3 ~/.richter/richter.db "DELETE FROM run_cache WHERE expires_at IS NOT NULL AND expires_at < datetime('now')"

# Clear all cache
sqlite3 ~/.richter/richter.db "DELETE FROM run_cache"
```

## Data Retention

| Data | Default TTL | Configurable |
|------|------------|--------------|
| Run output | 7 days | `retention.runs_days` in config |
| Events | 30 days | `retention.events_days` in config |
| Cache entries | Per-entry TTL | `cache_ttls` in config |
| Logs | Daily rotation | Automatic |

Manual pruning:
```bash
# Prune old events
sqlite3 ~/.richter/richter.db "DELETE FROM events WHERE created_at < datetime('now', '-30 days')"

# Prune old runs
sqlite3 ~/.richter/richter.db "DELETE FROM runs WHERE completed_at < datetime('now', '-7 days')"
```
