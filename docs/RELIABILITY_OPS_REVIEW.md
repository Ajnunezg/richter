# Reliability & Operations Review — Richter Codebase

**Date:** 2026-05-05
**Reviewer:** SRE/Operations Agent
**Scope:** Full codebase — all 4 crates, Makefile, CI, install/test scripts, troubleshooting docs
**Method:** Manual code review of ~8000 lines across 40+ source files, plus operational artefact analysis

---

## Overall Reliability/Ops Score: 4 / 10

### Rationale

Richter is a v0.1 codebase with solid architectural instincts but almost zero operational hardening. The core machinery (process supervision, scheduling, caching, event bus) is well-conceived, but every reliability layer a production system needs is either missing, stubbed out, or implemented as a placeholder. A single developer dogfooding this on their Mac would be fine. A team of 5+ engineers relying on it daily would encounter data loss, unbounded resource growth, and undebuggable failures within a week.

The score is not a criticism of the code quality — the code that exists is clean and well-structured. It's a reflection of operational maturity. At 0.1.0, that's expected. But the gap between "works on my machine" and "team can bet their workflow on it" is large.

---

## 1. What Would Most Likely Break in Production

### 1.1 Single-threaded SQLite becomes a choke point and data-loss vector

**File:** `crates/richter-core/src/db.rs:44` — `parking_lot::Mutex<rusqlite::Connection>`

There is exactly one database connection. Every read, write, and migration goes through it. Under concurrent agent load, the mutex serializes all DB access. The run-or-join hot path blocks on cache lookups. More critically: there is no WAL checkpoint management, no busy timeout, and no retry on `SQLITE_BUSY`. Under load, concurrent writes will fail silently or panic.

The scheduler completion hook (`run_manager.rs:338-390`) spawns a tokio task that loops every 200ms polling for exit codes and writes to the DB on completion — with no error recovery beyond a `tracing::warn!`. If the DB write fails, the cache entry is lost permanently. No retry, no fallback.

**What breaks:** Cache misses cascade into duplicate work. Run history disappears. Dashboard goes blank.

### 1.2 Orphaned processes after daemon crash

**File:** `crates/richter-daemon/src/supervisor.rs:267-270`

```rust
pub async fn check_orphans(&self) -> Vec<String> {
    Vec::new()
}
```

The orphan detection function is a hardcoded empty vec. If the daemon crashes (OOM kill, power loss, SIGKILL), child processes with `kill_on_drop(false)` continue running. On restart, the daemon has no mechanism to discover or reconcile these orphans. The LaunchAgent `KeepAlive=true` will restart the daemon, which then creates a fresh socket, generates a new auth token, and starts from scratch — while old builds/tests keep consuming CPU.

The shutdown drain in `main.rs` handles the graceful case, but ungraceful termination has zero recovery.

**What breaks:** Zombie `cargo build` processes consume CPU indefinitely. Resource limits become meaningless because the scheduler doesn't know about orphaned processes. User manually kills them via Activity Monitor.

### 1.3 Disk full silently breaks everything

**File:** `crates/richter-daemon/src/scheduler.rs:131` — `disk_free_bytes: 0`

The `ResourceMonitor` collects disk metrics but `disk_free_bytes` is hardcoded to `0` in the collector. There is no disk pressure threshold, no monitoring, and no alert.

SQLite WAL mode with no checkpoint management means WAL files grow unboundedly. The log files (`~/.richter/logs/daemon.log`) have no rotation — tracing-appender is in `Cargo.toml` dependencies but never wired to the actual tracing setup. In `main.rs:31-36`, tracing is configured with `fmt()` only, writing to stderr. The LaunchAgent plist redirects stdout/stderr to `daemon.log`, which will grow until the disk fills.

**What breaks:** Disk fills → SQLite writes fail → all state lost → daemon can't start → user has no idea why because `richter doctor` can't connect to the daemon.

### 1.4 Scheduler queue full = silent rejections with no alert

**File:** `crates/richter-daemon/src/scheduler.rs:240-243`

```rust
if queue.len() >= self.config.queue_max {
    warn!("Scheduler queue full, rejecting run {run_id}");
    return None;
}
```

When the queue hits 64 entries, new runs are rejected with `None`. The `RunManager` converts this to `RunOutcome::Rejected`. But there's NO notification, NO event emitted, and NO metric. The agent that submitted the command gets a rejection — but it doesn't know whether to retry, wait, or escalate. There's no backoff guidance in the response.

**What breaks:** Under heavy load, agents get rejected silently. The user sees "why isn't my build running?" with no explanation.

### 1.5 No retry or backoff anywhere

A systematic grep for `retry`, `backoff`, `circuit_breaker`, and `rate_limit` across the entire codebase found:

| Pattern | Result |
|---------|--------|
| `retry` logic | Webhooks module claims "retry logic" in its doc comment but implements zero retry |
| `backoff` | Not found anywhere |
| `circuit_breaker` | Only in `ModelCallBudget` — in-memory rate limiter, unused by any actual model call code |
| `rate_limit` | Only in `NotificationConfig` — a config value, no enforcement code |

Every external interaction (git commands in fingerprinting, DB writes, socket connections) is fire-and-forget. No retry, no exponential backoff, no jitter.

**What breaks:** Transient failures (git index locked, SQLITE_BUSY, socket temporarily unavailable) become permanent failures.

---

## 2. Missing Operational Capabilities (by severity)

### Critical (would block production adoption)

| Capability | Status | Evidence |
|-----------|--------|----------|
| **Log rotation / retention** | ❌ Missing | `tracing-appender` in deps but unused. LaunchAgent redirects to unbounded log file |
| **Database backup** | ❌ Missing | No backup mechanism, no VACUUM, no integrity check. `db.sqlite` is the sole source of truth |
| **Orphan reconciliation** | ❌ Stubbed | `check_orphans()` returns `Vec::new()`. Graceful shutdown is the only cleanup path |
| **Metrics/alerting** | ❌ Missing | `/metrics` appears in OpenAPI spec but is NOT in the router. No Prometheus endpoint. No alerting hooks |
| **Disk monitoring** | ❌ Broken | `disk_free_bytes: 0` hardcoded. No disk pressure handling |
| **Crash recovery** | ❌ Missing | No pidfile, no crash counter, no startup health self-check, no state reconstruction on restart |
| **Upgrade safety** | ❌ Missing | No config versioning. Schema migrations exist but no downgrade path. Binary replacement during daemon runtime is undefined behavior |

### High (serious operational pain)

| Capability | Status | Evidence |
|-----------|--------|----------|
| **Retry/backoff** | ❌ Missing | Zero retry logic across all I/O paths. Single-shot fire-and-forget everywhere |
| **Idempotency in critical ops** | ⚠️ Partial | Fingerprint-based dedup is inherently idempotent for runs. But DB inserts, config writes, and cache entries are not |
| **Graceful degradation** | ❌ Missing | Every failure mode is binary: works or doesn't. No partial functionality when DB is down or watcher fails |
| **Health check depth** | ⚠️ Shallow | `/health` returns static JSON. No DB connectivity check, no watcher liveness, no scheduler health |
| **Rate limiting enforcement** | ❌ Missing | Config has `rate_limit_per_minute` but no enforcement code |
| **Memory limits on child processes** | ❌ Missing | No cgroup, ulimit, or RSS monitoring. A runaway `jest --maxWorkers=100%` can OOM the machine |
| **Circuit breaker for external calls** | ❌ Missing | Model call budget exists but no actual calls use it. Fingerprint git commands have no circuit breaker |

### Medium (quality-of-life gaps)

| Capability | Status | Evidence |
|-----------|--------|----------|
| **Config validation** | ❌ Missing | TOML is parsed but values aren't validated. Negative TTLs, invalid paths, impossible thresholds all accepted silently |
| **Config hot-reload** | ❌ Missing | Config changes require daemon restart (`richter daemon restart`) |
| **Audit trail completeness** | ⚠️ Partial | Events exist but aren't persisted. Audit endpoint drains ephemeral broadcast channel — events older than channel capacity are lost |
| **Runbook automation** | ⚠️ Partial | Excellent Troubleshooting doc but all steps are manual. No `richter repair` or `richter recover` commands |
| **Startup time observability** | ❌ Missing | No startup phases logged with timing. Daemon startup is "sleep 300ms then hope it worked" |

---

## 3. Strengths in the Ops Model

Despite the gaps, Richter gets several things genuinely right for a v0.1:

### 3.1 Process supervision is the strongest component

**File:** `crates/richter-daemon/src/supervisor.rs`

- `nix`-based process group management with `setpgid()` and `killpg(SIGKILL)` — correct Unix semantics
- `kill_on_drop(false)` prevents accidental cleanup on handle drop
- Stall detection at 300s no-output with automatic kill
- 1MB output buffer cap prevents runaway memory
- Separate stdout/stderr reader tasks with timeout
- Proper `done_tx` watch channel for completion signaling

This is genuinely production-quality process management.

### 3.2 Graceful shutdown is well-designed

**File:** `crates/richter-daemon/src/main.rs:128-160`

- Dual signal handling (Ctrl-C + SIGTERM)
- 30-second drain period waiting for active runs
- Orphan reconciliation: marks orphaned runs in DB, kills remaining processes
- Socket file cleanup
- Mobile gateway gets its own shutdown signal

For the graceful path, this is solid.

### 3.3 Database schema versioning exists

**File:** `crates/richter-core/src/db.rs:568-614`

- `_schema_version` table with migration numbering
- `migration()` dispatch function with match arms
- v1 creates all 15+ tables with proper indexes and FKs
- v2 adds mobile companion tables
- `IF NOT EXISTS` guards on v2 tables

Having a migration engine at v0.1 is unusually forward-thinking.

### 3.4 Troubleshooting documentation is excellent

**File:** `docs/TROUBLESHOOTING.md`

Comprehensive, symptom-based, with concrete commands and expected outputs. Covers daemon startup, shims, PATH, permissions, MCP connections, agent detection, cache invalidation, resource deadlocks, and debug logging. This is better than many production systems.

### 3.5 Local-first, no cloud dependency

The architecture explicitly avoids cloud dependencies. All state is local. No telemetry. No phoning home. This eliminates an entire class of operational risks (API outages, auth key rotation, cloud billing surprises). The `RichterConfig` model providers are optional.

### 3.6 Event bus with coalescence

**File:** `crates/richter-daemon/src/event_bus.rs`

- `tokio::sync::broadcast` with 256 capacity
- Event coalescence for duplicate events within 250ms
- Filtered subscriptions by variant
- SSE streaming endpoint for real-time UI updates
- Lag detection with explicit lagged-event messages

This is a well-implemented in-process pub/sub that enables observability.

---

## 4. Specific Recommendations (ordered by impact/effort ratio)

### Immediate (before any team adoption)

1. **Wire `tracing-appender` for log rotation.** It's already in `Cargo.toml`. A 30-minute change:
   ```
   use tracing_appender::rolling::{RollingFileAppender, Rotation};
   let file_appender = RollingFileAppender::new(Rotation::DAILY, data_dir.join("logs"), "daemon.log");
   ```
   Solves the disk-fill problem and makes logs browsable.

2. **Add DB integrity check on startup.** A single `PRAGMA integrity_check` in `Database::open` after migrations. If it fails, log the error, back up the corrupted file, and create a fresh DB rather than silently corrupting state.

3. **Implement orphan reconciliation on startup.** Before the daemon accepts connections:
   - Scan for processes whose parent is this daemon's PID from a previous run (track via pidfile)
   - Kill any found orphans
   - Mark them as orphaned in the DB

   Pair with a pidfile at `~/.richter/daemon.pid` that the LaunchAgent or daemon writes on startup.

4. **Make `/metrics` actually exist.** It's advertised in the OpenAPI spec. Wire a real handler that exposes:
   - `richter_active_runs`
   - `richter_queued_runs`
   - `richter_cache_hits_total`
   - `richter_duplicates_prevented_total`
   - `richter_scheduler_queue_depth`
   - `richter_db_operation_duration_seconds` (histogram)
   - CPU/memory gauges

   This gives a team something to hook alerting into.

5. **Add disk pressure monitoring.** In `collect_snapshot()`, use `sysinfo::Disks::new_with_refreshed_list()` to get actual free bytes. Add a `disk_pressure_threshold` config (default 95%) and treat it like CPU/memory pressure.

### Short-term (within first month of team use)

6. **Add retry with exponential backoff for DB writes.** Wrap `conn.execute()` in a helper that retries on `SQLITE_BUSY` (3 attempts, 10ms/100ms/1s backoff). This alone would prevent most data loss.

7. **Add config validation.** In `load_config_file`, after parsing, validate:
   - Resource limits are non-zero
   - Cache TTLs are non-negative
   - Paths in `watched_folders` exist (warn if not)
   - CPU/memory thresholds are between 0 and 1

   Return a structured `Vec<ValidationError>` that `richter doctor` can display.

8. **Persist events to DB.** The event bus is ephemeral broadcast. Add a subscriber that writes events to the `events` table so the audit trail survives restarts. Batch writes (every 1s or 100 events) to avoid DB contention.

9. **Add startup health self-check.** Before the daemon reports "ready":
   - Verify the DB is accessible (simple SELECT 1)
   - Verify the socket is bound
   - Verify the watcher has at least one target (warn if not)
   - Log startup duration

   Only then emit `DaemonStatus { status: "running" }`.

10. **Add `richter recover` command.** A CLI subcommand that:
    - Kills orphaned processes (via `richter doctor --processes` output)
    - Removes stale socket files
    - Runs `PRAGMA integrity_check` on the DB
    - Clears cache if fingerprint schema version changed
    - Resets the scheduler queue

    Reduces "ask the SRE" to "run this command."

### Medium-term (production hardening)

11. **Connection pool for SQLite.** Switch from `parking_lot::Mutex<Connection>` to a pool (even a small one, 3-5 connections). Use `r2d2-sqlite` or switch to `sqlx` (already in workspace deps). WAL mode supports concurrent readers.

12. **Add migration downgrade paths.** Each `migration()` should come with a `downgrade()` function. Store the previous schema version before upgrading so rollback is possible.

13. **Config versioning.** Add a `version = 1` field to `RichterConfig`. On load, if the version is lower than current, run config migrations (add new fields with defaults, rename old fields).

14. **Binary upgrade safety.** Document the upgrade procedure:
    - Stop daemon → replace binary → start daemon
    - Or implement a zero-downtime upgrade via socket handoff (Unix `SCM_RIGHTS`)

    At minimum, the install script should stop the daemon before replacing the binary.

15. **Add backpressure to MCP transport.** `mpsc::unbounded_channel` in `StdioTransport` has no limit. Switch to bounded channels with a reasonable cap (1000 messages) and drop oldest on overflow with a warning.

---

## 5. Operational Runbook Assessment

| Artifact | Status | Notes |
|----------|--------|-------|
| `docs/TROUBLESHOOTING.md` | ✅ Excellent | Comprehensive, symptom-based, actionable |
| `docs/INSTALL.md` | ✅ Good | Clear steps, verification commands |
| `scripts/install.sh` | ✅ Good | Idempotent checks, colored output, verification step |
| `scripts/test.sh` | ✅ Good | Flexible flags, proper error handling |
| `scripts/build.sh` | ⚠️ Adequate | Basic, no caching, no incremental build support |
| `Makefile` | ⚠️ Minimal | 7 targets, no `make dev`, no `make docker`, no `make benchmark` |
| CI (`.github/workflows/ci.yml`) | ⚠️ Minimal | fmt + clippy + check + test on macOS only. No integration tests, no cross-platform, no coverage |
| `richter doctor` | ✅ Good | Covers daemon, shims, PATH, hooks, MCP, permissions, providers |
| `richter status` | ⚠️ Adequate | Shows active/queued counts + resource snapshot |
| Runbook for crash recovery | ❌ Missing | No documented procedure for daemon crash, DB corruption, or disk full |
| Runbook for upgrades | ❌ Missing | No documented upgrade procedure |
| Runbook for data recovery | ❌ Missing | No backup/restore documented |

---

## 6. Summary

Richter's architecture is sound. The process supervision, run-or-join engine, fingerprint deduplication, and event bus are well-designed and cleanly implemented. The troubleshooting documentation is unusually good for a v0.1.

But the gap between "architecturally sound" and "operationally safe" is measured in missing infrastructure: no log rotation, no metrics, no retry, no backup, no crash recovery, no config validation, no health check depth, no rate limiting enforcement. Every one of these is individually fixable, and several (log rotation, DB integrity check, orphan reconciliation) are 30-minute changes.

The codebase is **not ready for a team to bet their workflow on it** — but it's close enough that 1-2 weeks of focused operational hardening could bring it from 4/10 to 7/10. The hardest work (correct process management, schema versioning, graceful shutdown) is already done. What remains is plumbing.
