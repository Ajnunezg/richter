# Reliability & Operations Deep Review — Richter Codebase

**Date:** 2026-05-18
**Reviewer:** SRE/Operations Deep-Dive Agent
**Scope:** Full codebase — all crates, with focus on `richter-daemon` and `richter-core`
**Method:** Line-by-line code review of `main.rs`, `supervisor.rs`, `api.rs`, `run_manager.rs`, `db.rs`, `scheduler.rs`, `event_bus.rs`, `config.rs`, `webhooks.rs`, `watcher.rs`, plus cross-referencing existing reviews for accuracy

---

## Corrections to Existing Reviews

The existing `RELIABILITY_OPS_REVIEW.md` (scored **4/10**) contains several factual errors that this deep review must correct:

| Claim in Existing Review | Reality | Evidence |
|---|---|---|
| "Single-threaded SQLite via `parking_lot::Mutex<rusqlite::Connection>`" | **Wrong.** The DB uses `sqlx::SqlitePool` with `max_connections(8)` and WAL mode | `db.rs:44-52` — `SqlitePoolOptions::new().max_connections(8)` |
| "No WAL checkpoint management" | **Wrong.** `checkpoint_wal()` exists and is called on graceful shutdown | `db.rs:87-91`, `main.rs:411` |
| "No DB integrity check" | **Wrong.** `PRAGMA integrity_check` runs on every `Database::open` and fails hard if not "ok" | `db.rs:57-63` |
| "No busy timeout" | **Wrong.** `busy_timeout(5s)` is configured | `db.rs:49` |
| "`check_orphans()` returns hardcoded empty vec" | **Partially outdated.** The current implementation does actual orphan detection via `try_wait()` + cleanup | `supervisor.rs:540-567` |
| "No log rotation" | **Wrong.** `tracing-appender` is wired: daily rolling files + non-blocking writer + optional JSON format | `main.rs:85-112` |
| "No metrics endpoint" | **Wrong.** `/metrics` handler exists with `MetricsResponse` struct | `api.rs:883-898` |
| "No pidfile" | **Wrong.** pidfile logic exists with stale pid detection and double-start prevention | `main.rs:60-81` |
| "No startup DB integrity check" | **Wrong.** It's there and fails hard on corruption | `db.rs:57-63` |
| "No DB backup mechanism" | **Wrong.** `Database::backup()` exists via `VACUUM INTO`. Startup copies `richter.db.backup` | `db.rs:95-100`, `main.rs:131-135` |
| "No orphan reconciliation on startup" | **Wrong.** `list_active_runs()` marks orphaned runs as Failed on startup | `main.rs:160-183` |
| "`disk_free_bytes: 0` hardcoded" | **Wrong.** `collect_snapshot()` uses `sysinfo::Disks::new_with_refreshed_list()` | `scheduler.rs:108-116` |

**Net effect:** The existing review severely undervalues the codebase. Many of the "Critical" gaps it identified have been fixed since its 2026-05-05 review date (or were already fixed and the reviewer missed them). That said, significant gaps remain, but in different areas than reported.

---

## Corrected Overall Score: 6 / 10

The codebase has received meaningful operational hardening since the initial review. The core infrastructure (database pooling, integrity checks, pidfile, log rotation, orphan reconciliation on startup, WAL checkpoint on shutdown, metrics endpoint, disk monitoring) is now in place. What remains missing is the **second tier** of operational maturity: retry wiring, transaction safety, structured observability, config validation, and operational runbooks.

---

## 1. Production Readiness Assessment

### 1.1 What's Now Production-Ready

**Daemon startup is solid.** The pidfile prevents double-start with stale PID detection. The data directory gets `chmod 0700`. The auth token gets `chmod 0600` with verification. The database is integrity-checked, backed up, and migrations run automatically. Orphaned runs from previous crashes are reconciled. Log rotation is wired with daily rolling files and optional JSON format. *(main.rs:39-183)*

**Process supervision is genuinely production-quality.** `setpgid()` + `killpg(SIGKILL)` is correct Unix. Stall detection at 300s with automatic kill. 1MB output buffer cap. Separate stdout/stderr reader tasks. Proper completion signaling via watch channels. Command validation (length, forbidden characters). Dangerous env key blocking. *(supervisor.rs:1-700)*

**Database layer is well-architected.** `sqlx::SqlitePool` with 8 connections, WAL mode, foreign keys, 5-second busy timeout. Integrity check at open. `VACUUM INTO` backup. Schema versioning with migration functions. Row types with typed accessors. *(db.rs:1-600)*

**Auth is constant-time.** `subtle::ConstantTimeEq` is used for token comparison, token files get 0600 permissions with verification/correction at startup. *(api.rs:149-156, 185-230)*

### 1.2 What's Still Not Production-Ready

**No transactions anywhere.** Database operations are individual SQL statements with no transaction wrapping. The completion hook in `run_manager.rs:640-690` does: (1) write cache to in-memory LRU, (2) write cache to DB, (3) notify subscribers, (4) release scheduler, (5) remove from active maps — as five independent operations. If step 2 fails, steps 3-4 still run, creating inconsistency: the scheduler releases the permit, subscribers get notified with exit codes, but the cache entry is lost. On restart, the DB has no record, the in-memory cache is gone.

**No database write retry.** The `richter-core::retry` module exists with `BackoffConfig::db_writes()` (10ms/500ms/3-attempts), but **zero callers use it anywhere in the codebase.** Every `db.insert_*`, `db.update_*`, `db.acquire_lease` call is fire-and-forget — a single `SQLITE_BUSY` kills the operation permanently. This is the single most impactful reliability gap.

**Completion hook has no error recovery.** `run_manager.rs:640-690` — if `db.insert_cache_entry()` fails (line 685), the only consequence is `tracing::warn!`. The cache entry is lost. No retry, no fallback storage, no flagging of the data loss. The subscriber notification and scheduler release happen regardless.

**Blocking I/O in async context.** `CachedResult::is_fresh()` does `std::fs::metadata()` synchronously in a loop over changed files (`run_manager.rs:130`). Called from async contexts. A network filesystem or a repo with thousands of changed files will block the Tokio runtime.

**`std::fs::metadata` on the hot path without `spawn_blocking`.** Same issue exists in `main.rs` (pidfile reads, directory creation, DB backup), `plugin_runtime.rs:35` (`read_dir`), and `run_manager.rs:430` (reading cached output files from disk).

---

## 2. Logging Quality Assessment

### 2.1 What's Good

- **`tracing` is used consistently** across all daemon code — `info!`, `warn!`, `error!`, `debug!` with appropriate use.
- **Log rotation is wired:** daily rolling files via `tracing_appender::rolling::daily`, non-blocking writer, so log I/O never blocks the runtime. *(main.rs:85-99)*
- **Optional JSON format:** `RUST_LOG_FORMAT=json` switches to structured JSON output for machine consumption. *(main.rs:101-115)*
- **`EnvFilter` with sensible defaults:** `richter_daemon=info,richter=info` allows per-crate control. *(main.rs:107)*
- **Some structured fields exist:** `socket = %socket_path.display()`, `error = %e` in a few places. *(main.rs:323, 327)*

### 2.2 What's Missing

- **No request IDs / trace IDs.** Zero `#[instrument]` attributes anywhere. No `tracing::Span` for request correlation. When two agents submit commands simultaneously, there's no way to trace which log lines belong to which request. Every `/run_or_join` call is invisible in the logs beyond the event bus emission.
- **No structured fields on critical paths.** The completion hook logs `tracing::warn!("Failed to persist cache entry to DB: {e}")` but doesn't include the `run_id`, `cache_key`, or operation type. You'd have to correlate by timestamp.
- **Inconsistent log levels.** Cache hits are `debug!` (good), but queue rejections are `warn!` with no structured context about which agent was rejected or what the queue depth was. The scheduler logs `info!("Enqueued run...")` but the rejection is just `warn!("Scheduler queue full")`.
- **No operation timing.** No `tracing::info!` with `elapsed = ?` for DB operations, fingerprint computation, or process spawn. You cannot answer "how long does a typical DB write take?"
- **`_guard` drop risk.** The `tracing_appender` non-blocking guard (`_guard` in `main.rs:88`) must outlive the tracing subscriber. It does here because it's in `main()`, but refactoring that moves it into a sub-function would silently break log output.

**Score: 5/10** — The plumbing is there (rotation, JSON format), but the content is unparsable without manual timestamp correlation. No request tracing, no span trees, no operation timing.

---

## 3. Monitoring / Observability Assessment

### 3.1 Health Check — Partial Depth

`/health` now checks DB connectivity via `list_active_runs()` and watcher liveness via `watcher_healthy` AtomicBool. This is solid but incomplete:

- **No scheduler health check.** The health endpoint returns `"scheduler": "ok"` hardcoded. It doesn't check if the queue processor task is alive, or if permits are exhausted.
- **No disk space check in health.** The scheduler's `collect_snapshot()` now properly reads disk free bytes, but `/health` doesn't report disk pressure. A full-disk scenario would show "ok" with DB writes silently failing.
- **No memory/CPU pressure in health.** Resource snapshot exists but isn't surfaced in `/health`.

### 3.2 Metrics — Exists but Shallow

`/metrics` returns structured JSON with:
- `active_runs`, `queued_runs`, `cache_hits_today`, `duplicates_prevented`, `scheduler_permits_available`, `scheduler_queue_depth`

**Missing:**
- **No operation latencies.** No histograms for `run_or_join` P50/P95/P99, DB query duration, fingerprint computation.
- **No error rates.** No `richter_db_write_failures_total`, `richter_run_failures_total`, `richter_cache_misses_total`.
- **No process metrics.** No child process RSS, CPU, exit code distributions.
- **No Prometheus format.** It's JSON-only, not OpenMetrics text format. You can't point a Prometheus scraper at it.
- **No gauge for watcher event lag.** The event bus has lag detection but it's not surfaced as a metric.

### 3.3 Alerting — Non-existent

There are zero alerting hooks. The event bus emits `ResourcePressure`, `ConflictDetected`, and `ImportantEvent` — but nothing subscribes to these for alerting. The mobile gateway has a notification system but it's for the companion app, not for ops. No webhook firing on resource pressure. No integration with macOS Notification Center for critical events. No email/Slack/Discord integration.

**Score: 4/10** — Metrics exist but can't feed a standard monitoring stack. Health is shallow. Alerting is absent.

---

## 4. Resilience Pattern Inventory

### 4.1 What Exists

| Pattern | Location | Quality |
|---------|----------|---------|
| Process supervision + stall detection | `supervisor.rs` | ✅ Production-quality |
| Graceful shutdown with 30s drain | `main.rs:371-416` | ✅ Solid |
| Orphan reconciliation (startup + shutdown) | `main.rs:160-183, 387-406` | ✅ Now functional |
 | Scheduler backpressure (queue cap 64) | `scheduler.rs:240-243` | ⚠️ Returns `None` silently |
| Model call budget (circuit breaker-ish) | `api.rs:32-68` | ⚠️ Exists but unused by any actual model code |
| CORS origin allowlist | `api.rs:1019-1030` | ✅ Restrictive |
| Command validation (length, forbidden chars) | `supervisor.rs:162-178`, `api.rs:338-400` | ✅ Defense in depth |
| Dangerous env blocking | `supervisor.rs:194-200` | ⚠️ Deny-list, not allow-list |
| DB busy timeout | `db.rs:49` | ✅ 5 seconds |
| DB integrity check at startup | `db.rs:57-63` | ✅ Hard fail |
| WAL checkpoint on shutdown | `db.rs:87-91`, `main.rs:411` | ✅ |
| Idempotent cache insert (`ON CONFLICT` upserts) | `db.rs` various | ⚠️ Repo/agent upserts are idempotent; cache inserts use raw `INSERT` (no conflict handling) |
| Disk pressure check before scheduling | `scheduler.rs:228-234` | ✅ Now functional |

### 4.2 What's Missing

| Pattern | Impact | Evidence |
|---------|--------|----------|
| **Retry for DB writes** | Critical | `richter_core::retry` exists but is never called. Every DB write is single-shot. |
| **Retry for git commands** | High | `fingerprint.rs` calls `git` with no retry. A locked `.git/index` is a permanent failure. |
| **Async retry** | High | The retry module is synchronous (`std::thread::sleep`), so it can't be used from async contexts without `spawn_blocking`. |
| **Circuit breaker for external calls** | High | Model call budget is defined but disconnected. No circuit breaker for git, DB, or filesystem. |
| **Transactions** | Critical | Zero transaction usage. Multi-step operations are not atomic. |
| **Graceful degradation** | Medium | When DB is unavailable, the daemon doesn't fall back to in-memory-only mode. It just errors. |
| **Backpressure on event bus consumers** | Medium | Broadcast capacity 256, lagged consumers get dropped silently. |
| **Request timeout in API** | Medium | No `tower::timeout` or request-level deadlines on any API endpoint. A hung client holds the connection forever. |
| **Connection timeout for mobile gateway** | Low | Mobile gateway binds TCP with no SO_TIMEOUT or idle connection reaping. |

**The retry module is the most damning finding.** It's well-implemented (exponential backoff + jitter + purpose-specific configs like `db_writes()` and `git_commands()`), has tests, and is exported from `richter-core`. But not a single `use richter_core::retry` import exists in the entire codebase. It's a loaded gun that nobody fires.

---

## 5. Configuration and Secrets Management

### 5.1 Config Loading

- **TOML-based** with serde deserialization. Global config at `~/.richter/config.toml`, per-repo at `.richter/config.toml`. Merge overlay for repo-specific overrides. Defaults are comprehensive. *(config.rs)*
- **No config validation.** Negative TTLs, invalid paths, impossible thresholds (CPU threshold > 1.0), zero resource limits — all accepted silently. `load_config_file` parses TOML and returns. No `validate()` step.
- **No config versioning.** No `version` field. Adding a new required field breaks existing configs silently (serde defaults handle it, but you lose the signal that the config is out-of-date).
- **No config hot-reload.** Config is read once at startup. Changing `config.toml` requires `richter daemon restart`.
- **Environment variables are checked but not consistently.** `RICHTER_SOCKET`, `RICHTER_MOBILE_ENABLED`, `RICHTER_MOBILE_LAN`, `RICHTER_MOBILE_TLS`, `RICHTER_MOBILE_PORT`, `RUST_LOG`, `RUST_LOG_FORMAT` — but there's no unified env-var handling, no `--config` CLI flag for specifying a config path, and no documentation of the full env var surface.

### 5.2 Secrets Management

- **Auth token:** Generated at startup via `generate_auth_token()` — SHA256 of random bytes + salt, stored at `0600`. **No rotation.** No revocation list. If leaked (env dump, accidental log), the only remediation is to delete the file and restart. *(api.rs:185-230)*
- **Database:** The SQLite file at `~/.richter/richter.db` gets `0600` permissions with startup verification. This is good. *(main.rs:142)*
- **Data directory:** `~/.richter/` gets `0700` with startup correction. This is good. *(main.rs:44-56)*
- **Mobile gateway:** Ed25519 keys, TLS certificates stored in the data directory. The pairing secret uses a SHA256 hash. Session tokens have expiry. The implementation is careful and well-thought-out for the mobile surface.
- **Webhook secrets:** Stored as `Option<String>` in `WebhookConfig`. No encryption at rest. Any process that can read `~/.richter/` can extract webhook secrets. Webhooks are also **never actually delivered** — the `webhooks.rs` module only manages CRUD, no delivery loop exists.

**Score: 5/10** — Auth token and file permissions are solid. No rotation, no revocation, no config validation, no hot-reload.

---

## 6. Database Operational Concerns

### 6.1 Connection Pooling

The initial review's claim of "single `parking_lot::Mutex<Connection>`" is **wrong**. The current implementation uses `sqlx::SqlitePool` with 8 max connections, WAL mode, and a 5-second busy timeout. This is appropriate for a single-machine daemon. *(db.rs:44-52)*

### 6.2 Migration Safety

Migrations run sequentially via `run_migrations()` with a `_schema_version` table. Each migration version is recorded after successful execution. v2 uses `CREATE TABLE IF NOT EXISTS` for idempotent re-runs. *(db.rs:491-560)*

**Missing:**
- **No downgrade paths.** You cannot roll back a migration. If v3 introduces a destructive schema change, there's no `down_v3()`.
- **No migration within a transaction.** v1 has 26 DDL statements executed individually. If statement #23 fails, statements #1-22 are committed but #23-26 and the version record are not. The DB is in an inconsistent state with no rollback.
- **The version record is updated via DELETE + INSERT, not UPDATE.** `db.rs:542-548` — this works but is fragile. If the DELETE succeeds but the INSERT fails, the version is 0 and all migrations will re-run from v1.

### 6.3 Transaction Usage

**None.** Zero transaction wrappers. Every method creates, reads, or updates a single row or set of rows with no transaction boundary. The most dangerous example:

The completion hook in `run_manager.rs:640-690` does these logically-atomic operations:
1. Cache result in LRU
2. Insert cache entry in DB
3. Notify subscribers (send exit codes)
4. Release scheduler permit
5. Remove from active maps

If step 2 fails, steps 1, 3, 4, 5 still execute. The cache entry is in memory but not persisted. On restart, both caches are empty for this run.

### 6.4 WAL Concerns

- **WAL checkpoint on shutdown** — ✅ implemented
- **No periodic WAL checkpoint.** The WAL file can grow large between daemon restarts for long-running daemons. No periodic `PRAGMA wal_checkpoint(PASSIVE)` in the background.
- **No VACUUM scheduling.** No periodic `VACUUM` or `PRAGMA incremental_vacuum`. The DB will grow unboundedly, especially the `run_cache` table with large outputs. The existing `evict_expired_cache()` only deletes expired entries.

---

## 7. Graceful Shutdown and Crash Recovery

### 7.1 Graceful Shutdown — Solid

The shutdown sequence is well-designed:
1. Wait for Ctrl-C or SIGTERM
2. 30-second drain period checking `active_runs()` every 500ms
3. Abort the API server
4. Kill remaining active runs via supervisor
5. Mark orphaned runs in DB as Failed
6. WAL checkpoint
7. Remove socket and pidfile

This is genuinely good. *(main.rs:355-420)*

### 7.2 Ungraceful Crash Recovery — Now Partial

**Startup reconciliation** marks stale active runs as Failed. This is new since the initial review and works correctly. *(main.rs:160-183)*

**Remaining gaps:**
- **No crash counter.** If the daemon crashes on startup repeatedly (corrupted DB, missing binary), it will loop forever via LaunchAgent `KeepAlive=true` with no backoff. After 10 crashes, the DB integrity check or pidfile logic should trigger a "safe mode" or halt.
- **Pidfile race condition.** Between checking `pidfile_path.exists()` and writing the pidfile, another process could start. The `libc::kill(old_pid, 0)` check is good but not atomic. This is a minor theoretical concern for a single-user Mac daemon.
- **No startup duration logging.** The daemon logs "ready" after a 300ms sleep, but doesn't measure how long initialization took. No `tracing::info!` with `startup_duration_ms`.

### 7.3 SIGKILL Recovery — Still Weak

If the daemon receives SIGKILL (OOM, `kill -9`):
- Active child processes continue running (correct: `kill_on_drop(false)`)
- Pidfile remains on disk (good: next startup detects stale)
- Orphaned runs in DB get marked Failed (good)
- **But the actual child processes are not killed on restart.** The startup orphan reconciliation marks DB rows as Failed, but doesn't find and kill the still-running PIDs. You get zombie `cargo build` processes consuming CPU until manually killed.

The `supervisor.check_orphans()` method now does real detection (checks if child has exited without cleanup), but it's **never called on startup in main.rs.** It exists as a method but isn't wired into the startup sequence.

---

## 8. API Robustness

### 8.1 Input Validation — Good

`RunOrJoinRequest::validate()` checks:
- Command length ≤ 4096, non-empty, no forbidden chars
- Repo length ≤ 4096, no forbidden chars
- Env key ≤ 256, value ≤ 4096, max 100 entries
- Classification/resource_class ≤ 64 chars

This is defense-in-depth since `supervisor.rs` has its own validation. ✅

### 8.2 Auth — Good

Constant-time comparison via `subtle::ConstantTimeEq`. Token at 0600 with startup verification. ✅

### 8.3 Error Handling — Inconsistent

- **Run-or-join errors** return `StatusCode::INTERNAL_SERVER_ERROR` with `{"error": "..."}`. No error classification (transient vs permanent), no retry hints, no request IDs.
- **SSE lag events** are logged with `warn!` and sent as `{"lagged": N}` to the client. This is reasonable.
- **Audit endpoint** is a thin wrapper over `event_bus.subscribe_all().try_recv()` — it drains whatever events are buffered, which means it rarely returns useful data unless called right after events fire. It's not a real audit log.

### 8.4 Rate Limiting — Config Only

`NotificationConfig::rate_limit_per_minute` is a config value that is never enforced. The mobile gateway has a token-bucket rate limiter (`mobile_gateway.rs:125-170`) that actually works, but it's isolated to the mobile TCP surface. The Unix socket API has zero rate limiting — a misbehaving agent could submit 10,000 `run_or_join` requests per second.

### 8.5 Request Timeouts — None

No `tower::timeout::TimeoutLayer`, no `tokio::time::timeout` wrapping API handlers. A single hung handler (e.g., a DB write that blocks for 5s on busy timeout) holds the connection open indefinitely. With `max_connections(8)` on the pool, 8 concurrent slow queries can starve all API endpoints.

---

## 9. Resilience Anti-Patterns Found

### 9.1 Completion Hook Race Condition

`run_manager.rs:640-690` — The completion hook is a `tokio::spawn` with no error boundary. If the spawned task panics (e.g., `db.insert_cache_entry` returns an unexpected error type that's unwrapped), the entire completion notification is lost. Subscribers waiting on `watch::Sender` will wait forever. The scheduler permit is never released.

### 9.2 Cache Invalidation is Best-Effort with No Logging

`run_manager.rs:567-577` — `invalidate_repo_cache()` rebuilds the cache by scanning active runs, but uses `unwrap_or(true)` for entries with no matching active run, meaning it conservatively keeps entries that might be stale. This is correct behavior, but it's undocumented and the `true` default is a silent choice.

### 9.3 Unbounded Completed Children Map

`supervisor.rs` — `completed: Arc<DashMap<String, CompletedChild>>` grows without bound. Every completed run record stays in memory forever (or until the daemon restarts). There's no eviction, no TTL, no cap. A daemon running 10,000 runs per day accumulates 10,000 `CompletedChild` structs in memory per day. Each has an `output: String` field that could be up to 1MB.

**Evidence:** No `completed.remove()` or `.retain()` calls exist anywhere in `supervisor.rs`.

### 9.4 Synchronous Git Commands in Async Context

`run_manager.rs:648-653` — The completion hook runs `std::process::Command::new("git").args(["diff", "--name-only", "HEAD"])` synchronously inside a `tokio::spawn`. This blocks the Tokio runtime on git I/O. If the repo is large or the git index is locked, this blocks a worker thread.

### 9.5 Webhook System is a No-Op

`webhooks.rs` manages webhook CRUD but **never delivers webhooks.** No delivery loop, no queue, no HTTP client. The doc comment says "Uses tokio for async delivery with retry logic" but this is aspirational, not implemented. This was called out in the initial review as "claims retry logic" — it's still true.

---

## 10. Deployment and Rollback Readiness

- **No binary upgrade procedure.** The install script (`scripts/install.sh`) installs the binary but doesn't stop the daemon first. Replacing `~/.richter/bin/richterd` while the daemon is running produces undefined behavior on macOS (the old binary's mmap'd pages persist, new connections use the new binary, signal handlers may be inconsistent).
- **No version compatibility check.** The daemon doesn't verify that the CLI's version matches its own. A v0.2 CLI talking to a v0.1 daemon may produce unexpected JSON shapes.
- **No graceful restart.** There's no `richter daemon restart` that does stop-then-start with drain. The current approach is kill + wait-for-launchd-restart, which works but loses the 30-second drain period.
- **Schema migrations have no rollback.** If a migration breaks something, the only recovery is to restore from `richter.db.backup` (which is taken at startup) and downgrade the binary.

---

## 11. Operational Clarity

### 11.1 What's Good

- **`docs/TROUBLESHOOTING.md`** is genuinely excellent — symptom-based, concrete commands, expected outputs. 596 lines covering daemon startup, shims, PATH, permissions, MCP, cache invalidation, resource deadlocks, and debug logging.
- **`richter doctor`** CLI command provides comprehensive health checks.
- **`/openapi.json`** gives a machine-readable API spec.
- **`/onboard`** endpoint guides first-run setup.

### 11.2 What's Missing

- **No runbook for crash recovery.** What to do when the daemon crashes, the DB is corrupted, or the disk is full. The Troubleshooting doc covers "daemon not starting" but not "daemon crashed mid-run."
- **No runbook for upgrades.** The correct procedure (stop daemon → replace binary → start daemon) is not documented.
- **No runbook for data recovery.** `richter.db.backup` exists but there's no documented procedure for restoring it.
- **No `richter recover` command.** The Troubleshooting doc has steps but all are manual. No single command to: kill orphans, remove stale sockets, check DB integrity, clear stale cache, reset scheduler queue.
- **No operational metrics interpretation.** What does `queued_runs: 64` mean? Should you be worried? There's no baseline or threshold guidance.

---

## 12. Recommendations Ranked by Priority

### P0 — Fix Immediately (data loss risk)

1. **Wire `richter_core::retry` into DB writes.** Every `db.insert_*`, `db.update_*`, and `db.acquire_lease` call should be wrapped in `retry(..., &BackoffConfig::db_writes())`. The module exists and is tested. This is a 2-hour change that prevents 90% of transient data loss.

2. **Fix the completion hook to be resilient.** `run_manager.rs:640-690` — add `std::panic::catch_unwind` around the spawned task, or restructure so that scheduler release and subscriber notification only happen after DB persistence succeeds. If DB write fails, retry 3 times, and if it still fails, set an `in_memory_only` flag on the cache entry so it's not assumed to be persistent.

3. **Add transaction wrapping for logical operations.** The `run_or_join → start_new → completion hook` flow should be wrapped in a DB transaction where the cache insert and run status update are atomic.

4. **Wire `supervisor.check_orphans()` into startup.** It's implemented but never called from `main.rs`. Add it after the DB orphan reconciliation to also kill any still-running child processes.

### P1 — Fix Before Team Adoption (operability risk)

5. **Kill completed children after a retention period.** `supervisor.rs` — add a periodic task (every 10 minutes) that removes `CompletedChild` entries older than 30 minutes. This prevents unbounded memory growth.

6. **Add request IDs / `#[instrument]` to API handlers.** At minimum, add `#[instrument(skip(state))]` to `run_or_join_handler` and `health_handler`. Generate a UUID per request and include it in all log lines. This makes 95% of debugging possible.

7. **Add `tower::timeout::TimeoutLayer` to the API router.** 30-second request timeout. This prevents hung requests from consuming connections.

8. **Add API rate limiting.** Even a simple token-bucket per auth token (60 requests/minute) would prevent a misbehaving agent from drowning the daemon.

9. **Move blocking I/O off the Tokio runtime.** `CachedResult::is_fresh()`, the git diff command in the completion hook, and `std::fs::read` in DB cache retrieval should all use `tokio::task::spawn_blocking`.

10. **Add periodic WAL checkpoint.** Every hour, run `PRAGMA wal_checkpoint(PASSIVE)` in the background. This prevents WAL file growth for long-running daemons.

### P2 — Improve Before Production Scale (quality-of-life)

11. **Add config validation.** After parsing `config.toml`, validate: resource limits > 0, thresholds between 0 and 1, watched folders exist, TTLs are non-negative. Return `Vec<ValidationError>`.

12. **Add Prometheus-format `/metrics`.** The JSON endpoint is fine for ad-hoc queries, but a `text/plain; version=0.0.4` OpenMetrics endpoint makes the daemon observable by standard tooling.

13. **Add startup crash backoff.** Track consecutive startup failures in the pidfile. After 5 consecutive failures, log a warning and don't restart (let LaunchAgent keep the daemon down). Require manual `richter daemon start` to clear.

14. **Implement or remove webhook delivery.** The `webhooks.rs` module claims delivery but does none. Either implement it (with the existing `BackoffConfig`) or remove the `/webhooks` routes and the misleading doc comment.

15. **Document the env var surface.** Create a reference table of all `RICHTER_*` environment variables, their defaults, and their effects. Currently scattered across `main.rs`, `api.rs`, and `mobile_gateway.rs`.

16. **Add health check depth.** `/health` should include: disk free bytes, CPU/memory percentages, scheduler queue depth, DB pool connection count, and WAL file size. The existing `components` field is a good start but needs these values.

17. **Add a `richter recover` CLI command.** Automated: kill orphans, remove stale sockets, integrity-check DB, clear stale cache, reset scheduler queue, rotate auth token.

---

## 13. Reliability/Ops Scorecard

| Dimension | Score | Notes |
|-----------|-------|-------|
| Daemon startup / crash recovery | **7/10** | Pidfile, DB integrity, orphan reconciliation, permissions. Missing: crash counter, orphan killing |
| Process supervision | **8/10** | Best module in the codebase. Missing: sandbox, completed-children eviction |
| Graceful shutdown | **8/10** | 30s drain, DB reconciliation, WAL checkpoint. Solid |
| Logging / tracing | **5/10** | Rotation + JSON format wired, but zero request IDs, no spans, no timing |
| Monitoring / observability | **4/10** | Basic metrics, shallow health, no Prometheus format, zero alerting |
| Resilience patterns | **4/10** | Retry module exists but unwired. No transactions. No circuit breakers. Backpressure is queue-or-die |
| Database operations | **7/10** | Pool, WAL, busy timeout, integrity check, backup. Missing: transactions, periodic WAL, VACUUM |
| Configuration / secrets | **5/10** | Good permissions, no validation, no rotation, no hot-reload |
| API robustness | **6/10** | Good auth + validation. Missing: rate limiting, request timeouts, error classification |
| Operational clarity | **6/10** | Excellent Troubleshooting doc. Missing: runbooks, recover command, upgrade procedure |
| Deployment / rollback | **3/10** | No upgrade procedure, no version compat, no migration rollback |

**Overall: 6/10**

The codebase has matured significantly since the initial 4/10 review. The core infrastructure is solid — process supervision, database lifecycle, auth, graceful shutdown, and startup hardening are all genuinely production-quality. The gap is now in the "plumbing" layer: retry wiring, transaction boundaries, request tracing, and operational tooling. These are individually small changes but collectively determine whether a production incident is resolved in 5 minutes or 5 hours.

The single most impactful fix is wiring the existing retry module into DB operations. It would take 2 hours and moves the score from 6 to 7.
