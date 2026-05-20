# RichtaMista

Richter Remediation Roadmap: 64 → 95

Total estimated effort: 8-10 weeks for a strong Rust engineer.
Not all phases are sequential — some parallelize naturally.

---

## Phase 0: Critical Security Fixes (Days 1-2)

Non-negotiable. Ship these before anything else.

### 0.1 — Constant-time auth token comparison
- `api.rs`: replace `token == expected` with `subtle::ConstantTimeEq`
- `mobile_gateway.rs`: same fix for pairing token
- Add `subtle` to `Cargo.toml`
- Effort: 1 hour

### 0.2 — Auth token file permissions
- `main.rs:62`: replace `std::fs::write` with `generate_auth_token()` from `api.rs` (already does 0600)
- Add startup verification: assert `token_path` permissions are 0600, warn if not
- Apply same to `richter.db` on creation
- Effort: 2 hours

### 0.3 — Socket path TOCTOU fix
- Move default socket from `/tmp/richter.sock` to `~/.richter/daemon.sock` (matches docs)
- `~/.richter/` is user-owned, not world-writable — eliminates the symlink race
- Update `main.rs`, `client.rs`, and all docs that reference the old path
- Effort: 2 hours

### 0.4 — Env variable injection guard
- In `supervisor.rs` spawn, block dangerous env keys: `PATH`, `LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `LD_LIBRARY_PATH`
- Log a warning when an agent tries to inject these
- Add a configurable allowlist/denylist in `config.rs`
- Effort: 4 hours

### 0.5 — File permissions hardening
- At daemon startup, `chmod 0700 ~/.richter/` and `chmod 0600` on all sensitive files
- Add this to the startup sequence in `main.rs`
- Effort: 1 hour

Milestone: All critical and high-severity security findings resolved. Score moves from 7.0 to 8.0 on security.

---

## Phase 1: Type System Unification (Weeks 1-2)

The single highest-leverage refactoring in the entire codebase. This unlocks compiler-enforced correctness across every module boundary.

### 1.1 — Define `RichterError` enum in `richter-core`

Create new file `richter-core/src/error.rs`:
- `RichterError::Database { source: rusqlite::Error, query: String }`
- `RichterError::Config { path: PathBuf, reason: String }`
- `RichterError::Classification { command: String, reason: String }`
- `RichterError::Fingerprint { command: String, reason: String }`
- `RichterError::NotFound { entity: String, id: String }`
- `RichterError::InvalidState { ... }`

- Use `thiserror` (already in Cargo.toml, currently unused)
- Keep `anyhow` for daemon binary and CLI only
- Effort: 1 day

### 1.2 — Type the DB layer
- `db.rs`: convert all method signatures from `&str` to typed params
  - `insert_run(id: RunId, repo_id: RepoId, classification: CommandClass, ...)`
- Convert `RunRow.status: String` to `RunStatus`, `RunRow.classification: String` to `CommandClass`
- Add `impl FromRow for RunStatus` and `impl FromRow for CommandClass` with proper error variants
- This is the largest single task in the phase. ~30 methods to update.
- Effort: 3 days

### 1.3 — Type the daemon inter-module boundaries
- `RunSpec.classification: String` becomes `CommandClass`
- `RunSpec.resource_class: String` becomes `ResourceClass`
- `RunSpec.repo: String` becomes `PathBuf`
- `RunSpec.run_id: String` becomes `RunId`
- `scheduler.acquire(run_id: &str, repo: &str, ...)` becomes typed params
- `run_manager.run_or_join(spec: RunSpec)` — already typed if RunSpec is fixed
- Effort: 2 days

### 1.4 — Delete duplicate `ResourceClass` from daemon
- `scheduler.rs` re-declares `ResourceClass` with different variants than `richter-core/src/resource.rs`
- Delete the daemon copy, import from core
- Fix any variant mismatches
- Effort: 4 hours

### 1.5 — Unify the dual fingerprint systems
- Pick ONE: the `fingerprint.rs` BLAKE3 system (more comprehensive inputs, faster algorithm, used by ADRs)
- Delete `run_manager.rs::CommandFingerprint` with its SHA-256
- Refactor `run_manager.rs` to use `richter_core::fingerprint::fingerprint()`
- The BLAKE3 hash becomes the cache key, the dedup key, everything
- This fixes the cache inconsistency bug where DB cache hits returned empty output (different keys)
- Effort: 2 days

Milestone: Every module boundary is compiler-verified. No more string-matching on "test", "build", "running". Score moves from 7.5 to 8.5 on code quality, from 7 to 8.5 on architecture.

---

## Phase 2: Reliability and Ops Foundation (Weeks 2-3)

Making the daemon survive real production use.

### 2.1 — Retry with exponential backoff
- Add `richter-core/src/retry.rs`:
  - `async fn retry<T, E>(op: impl Fn() -> Result<T, E>, max_attempts: u3, backoff: BackoffConfig) -> Result<T, E>`
  - Exponential backoff with jitter: 10ms, 100ms, 1s
- Wrap all DB write operations: `insert_run`, `update_run_status`, `insert_cache_entry`, `insert_event`
- Wrap git commands in `fingerprint.rs`
- Effort: 1 day

### 2.2 — Orphan reconciliation on startup
- Write pidfile (`~/.richter/daemon.pid`) at startup
- On startup, check for previous pidfile. If exists and process is dead, proceed. If alive, another daemon is running.
- Query DB for runs with status `"running"` — these are orphans from a crash
- Kill any child processes belonging to the old PID (using `pgrep -P`)
- Mark orphaned runs in DB with status `"orphaned"` and timestamp
- Effort: 1.5 days

### 2.3 — Replace polling loops with event-driven wakeups
- `scheduler.rs acquire()`: replace 100ms polling with `tokio::sync::Notify`
  - When a permit releases (on `ActivePermit` drop), notify the queue
  - When resource pressure changes, notify
- `run_manager.rs start_new()`: replace 200ms exit_code polling with subscription to `supervisor.done_tx` watch channel
  - The watch channel already exists. Just subscribe to it instead of polling.
- Effort: 2 days

### 2.4 — DB operational hardening
- Add `PRAGMA busy_timeout = 5000` on connection open
- Add `PRAGMA integrity_check` on startup — fail fast if corrupted
- Add `PRAGMA wal_checkpoint(TRUNCATE)` on graceful shutdown
- Copy `richter.db` to `richter.db.backup` on startup before opening
- Effort: 1 day

### 2.5 — Logging and observability
- Wire `tracing-appender` (already in Cargo.toml for daemon) with daily rotation, 7-day retention
- Add JSON output option behind `RUST_LOG_FORMAT=json` env var
- Wire the `/metrics` endpoint (already in OpenAPI spec at `api.rs:534`):
  - `active_runs`, `queued_runs`, `cache_hits_total`, `cache_misses_total`
  - `scheduler_global_permits_available`, `scheduler_queue_depth`
  - `db_mutex_wait_time_ms` (histogram)
  - `process_spawn_total`, `process_kill_total`
- Effort: 2 days

### 2.6 — Graceful degradation
- Watcher init failure: log a WARNING (not silent), set a `watcher_healthy: false` flag in state
- DB write failure: retry (2.1), then queue in-memory and flush later
- Cache lookup failure: warn, proceed with new run (already does this, just needs the warning)
- Add `/health` endpoint that returns component status (db, watcher, scheduler)
- Effort: 1 day

Milestone: Zero retry becomes "retry everywhere." Polling becomes event-driven. Crash recovery exists. Observability is real. Score moves from 5 to 7.5 on reliability/ops.

---

## Phase 3: Performance and Scalability (Weeks 3-4)

Targeting 20+ concurrent agents, 50+ repos, weeks of uptime.

### 3.1 — Async database layer
- Move from `rusqlite` (blocking) to `sqlx` (async, already in Cargo.toml but unused)
- Connection pool: `SqlitePool::connect()` with `max_connections = 4`
- All DB methods become async, no more `parking_lot::Mutex` around the connection
- This is the single biggest scalability unlock — eliminates the DB mutex bottleneck
- Effort: 3 days (migration is mechanical but touches 30+ methods)

### 3.2 — Async fingerprinting
- `fingerprint.rs`: move git commands to `tokio::process::Command` (async)
- Run independent git commands concurrently (rev-parse, diff-index, etc.)
- Cache toolchain versions (`rustc --version`, etc.) with 60s TTL — they don't change mid-session
- Effort: 1.5 days

### 3.3 — Fix DB cache completeness
- `run_manager.rs:283`: read `output_path` from `run_cache` and serve the actual compressed output
- This makes DB cache hits return full results, not just exit codes
- Add output decompression and streaming for cached results
- Effort: 1 day

### 3.4 — Add pagination to list queries
- `list_runs_by_repo(repo, limit, offset)`, `list_events(filters, limit, offset)`
- Default limit: 50, max: 500
- Add cursor-based pagination for the SSE event stream
- Effort: 1 day

### 3.5 — Fix file watcher
- `find_repo_for_path()`: replace linear scan with sorted prefix lookup (binary search or small trie)
- Return longest matching prefix (fixes the wrong-repo bug)
- `should_include()`: replace hand-rolled glob with `globset` crate
- Fix `add_target()` to actually register with the notify watcher (expose method or restructure)
- Effort: 2 days

### 3.6 — Bound MCP channels
- `transport.rs`: add capacity bounds (1024) to all 5 channels
- Apply backpressure: return `ChannelFull` error instead of growing memory
- Effort: 4 hours

### 3.7 — Fix concurrency bugs
- Fix the dedup race condition (re-enable `concurrent_agent_e2e.rs:19`)
- Fix `can_run_immediately()` to check same-class concurrency, not all-run concurrency
- Make `supervisor.kill()` properly async (remove `block_on` in async context)
- Fix `cache_hits_today` to actually reset at midnight
- Effort: 2 days

Milestone: System handles 20+ agents, 50+ repos without degradation. All known race conditions fixed. Score moves from 6 to 8.5 on performance.

---

## Phase 4: Mobile Security Implementation (Weeks 4-6)

The largest single workstream. You wrote the spec. Now build it.

### 4.1 — TLS termination on mobile gateway
- Generate self-signed TLS cert on first startup, store in `~/.richter/mobile/`
- Bind `localhost:9777` only (not `0.0.0.0`) by default. LAN access only via explicit opt-in.
- Use `tokio-rustls` or `axum-server` with TLS acceptor
- Pin cert in mobile app
- Effort: 3 days

### 4.2 — Ed25519 device authentication
- Implement the pairing ceremony from `MOBILE_SECURITY.md`:
  - QR code generates 256-bit pairing secret with 120s window
  - Device generates Ed25519 keypair, sends public key to daemon
  - Daemon stores device public key with scope and expiry
- Per-request signing: device signs `(timestamp + method + path + body_hash)` with private key
- Daemon verifies signature with stored public key
- Effort: 5 days

### 4.3 — Replay protection
- Timestamp validation: reject requests older than 60s
- Bloom filter for seen request nonces (in-memory, ~10K entries, rotates daily)
- Effort: 1 day

### 4.4 — Scope enforcement
- Wire `device_has_scope()` into mobile auth middleware
- Per-device scopes: `readonly`, `run_commands`, `approve_actions`
- Default new devices to `readonly`
- Effort: 1 day

### 4.5 — Rate limiting
- Token bucket per device: 60 req/min default
- Return `429 Too Many Requests` with `Retry-After` header
- Effort: 1 day

### 4.6 — Wire mobile endpoints to real data
- Replace hardcoded zeros in `now_handler` with actual scheduler/monitor data
- Wire approve/deny handlers to real decision system
- Persist device registrations in SQLite (schema already exists in migration v2)
- Effort: 2 days

Milestone: Mobile gateway matches the security spec. Docs/code divergence on mobile is zero. Score moves from 3 to 8 on mobile security, from 7.0 to 8.5 overall on security.

---

## Phase 5: Testing and CI Excellence (Weeks 6-7)

Making the test suite match the codebase's ambition.

### 5.1 — Fix and expand integration tests
- Re-enable and fix `concurrent_agent_e2e.rs` (the dedup race)
- Add cross-crate integration tests: CLI to daemon to DB roundtrip
- Add MCP server end-to-end test (stdio transport, tool dispatch, resource queries)
- Effort: 3 days

### 5.2 — Stress testing
- Add `tests/stress_20_agents.rs`: simulate 20 concurrent agents, 5 repos, mixed command types
- Assert: no deadlocks within 60s, all runs complete, cache hit rate > 30%
- Add `tests/stress_long_running.rs`: simulate 24h of continuous operation (compressed)
  - Verify no memory growth beyond 2x baseline, WAL file stays < 50MB
- Effort: 2 days

### 5.3 — Smoke tests in CI
- Add CI job: start daemon, hit `/health`, hit `/status`, stop daemon
- Add `richter doctor` to CI — verify it passes on a fresh install
- Effort: 1 day

### 5.4 — Fuzz the parser surface
- Add `cargo-fuzz` targets for: classifier input, importance parser output, redaction patterns
- Run fuzzing in CI nightly (not blocking)
- Effort: 2 days

### 5.5 — Security test suite
- Test redaction against known secret formats (use fixtures/secrets-demo + add more)
- Test auth timing: assert constant-time comparison within statistical noise
- Test path traversal attempts: `../`, encoded variants, symlinks
- Test mobile gateway: replay attacks, expired tokens, wrong scope
- Effort: 2 days

Milestone: Test suite is comprehensive, not just present. CI catches real bugs. Score moves from 5.5 to 8 on testing.

---

## Phase 6: Release Engineering and Polish (Weeks 7-8)

Making this a product, not just a project.

### 6.1 — Release pipeline
- Add `scripts/release.sh`: bump version, generate changelog from conventional commits, tag
- CI job: on tag, build release binaries (macOS arm64 + x86_64), notarize with Apple
- Add `Cargo.toml` version check in CI (no unpublished version changes)
- Effort: 2 days

### 6.2 — DB migration robustness
- Add migration rollback support (down migrations)
- Test migration chain: v1 to v2 to v3 and v3 to v2 to v1
- Add `richter db backup` and `richter db restore` commands
- Effort: 2 days

### 6.3 — Docs/code reconciliation audit
- Systematically fix every docs/code mismatch found in diligence:
  - Socket path: `~/.richter/daemon.sock` everywhere
  - DB name: `richter.db` everywhere
  - Redaction format: decide on `[REDACTED:type]` or `[REDACTED]`, be consistent
  - Mobile security: docs match implementation after Phase 4
- Add CI check: grep docs for code references, verify they match reality
- Effort: 1 day

### 6.4 — Webhook delivery with SSRF protection
- Implement actual webhook delivery (async HTTP POST with retry)
- SSRF protection: deny private IPs, link-local, localhost, metadata endpoints
- HMAC signing with webhook secret
- Effort: 2 days

### 6.5 — Plugin integrity verification
- SHA-256 hash of plugin binary stored in manifest
- Verify hash before execution
- Warn on unsigned plugins, refuse tampered ones
- Effort: 1 day

### 6.6 — Redaction gaps
- Add `github_pat_` pattern for fine-grained PATs
- Add bare `BEGIN PRIVATE KEY` (PKCS#8)
- Add Kubernetes secrets, SendGrid, Twilio, Heroku, DigitalOcean patterns
- Add entropy heuristic for catch-all
- Effort: 1 day

Milestone: The codebase is a product. It has a release pipeline, protected webhooks, complete redaction, zero docs/code drift. Score moves from 6.5 to 9 on overall professionalism.

---

## Phase 7: The Last 0.5 — World-Class Touches (Weeks 8-10)

These are the things that separate "very good" from "holy shit, this is impressive."

### 7.1 — Structured concurrency for supervisor
- Use `tokio::task::JoinSet` instead of individual `tokio::spawn` for child process readers
- Guarantee all spawned tasks are cleaned up on shutdown
- Add max runtime limit (configurable, default 30 min) alongside stall detection
- Effort: 1 day

### 7.2 — Prepared statement cache
- Wrap rusqlite connection with `CachedStatement` for hot queries
- Or: if Phase 3.1 moved to sqlx, this comes for free
- Effort: 0.5 days (if rusqlite), 0 (if sqlx)

### 7.3 — Config validation
- `config.rs`: add `validate()` method that checks ranges
  - TTLs: 0-86400 seconds
  - CPU threshold: 0.0-1.0
  - Concurrency limits: 1-64
- Return typed errors on invalid config (uses Phase 1.1 `RichterError`)
- Add `richter config validate` command
- Effort: 1 day

### 7.4 — OpenAPI spec generation
- Use `utoipa` or `aide` to generate OpenAPI spec from Axum handlers
- Include in docs/ and validate in CI
- This is what makes the API surface discoverable for the SwiftUI app and mobile SDK
- Effort: 2 days

### 7.5 — Contribution guidelines hardening
- Expand `CONTRIBUTING.md` with: coding conventions, PR checklist, required test coverage
- Add `danger` or `prlint` for PR size checks
- Document the typed migration as a convention
- Effort: 1 day

### 7.6 — Performance regression tests
- Add `tests/bench_run_or_join.rs` — criterion benchmark for the hot path
- Assert: `run_or_join` cache hit < 1ms, cache miss < 50ms (excluding command execution)
- Run in CI on main branch, fail on > 20% regression
- Effort: 1 day

### 7.7 — ADR for every major decision going forward
- ADR 0009: Why sqlx over rusqlite (if Phase 3.1 is done)
- ADR 0010: Typed error hierarchy design
- ADR 0011: Event-driven scheduler wakeup
- Keep the discipline you've already established
- Effort: ongoing, ~1 hour per ADR

Milestone: The codebase is at the level where a Series A CTO reads it and finds nothing to criticize. Just clean, disciplined, production-grade Rust with genuine taste. Score: 9.5/10.

---

## Score Trajectory

| Phase | After | Overall Score | Key Unlock |
|---|---|---|---|
| Current | — | 64/100 | Baseline |
| Phase 0 | Day 2 | 68/100 | Security criticals fixed |
| Phase 1 | Week 2 | 75/100 | Type system unified, fingerprints unified |
| Phase 2 | Week 3 | 80/100 | Retry, crash recovery, observability |
| Phase 3 | Week 4 | 85/100 | Async DB, scalability, race conditions fixed |
| Phase 4 | Week 6 | 89/100 | Mobile security matches docs |
| Phase 5 | Week 7 | 92/100 | Test suite is comprehensive |
| Phase 6 | Week 8 | 94/100 | Release pipeline, docs/code consistency |
| Phase 7 | Week 10 | 95/100 | World-class polish |

---

## Critical Path

The phases that matter most for impact-per-day:

1. Phase 1 (type system) — unlocks everything else. Do this first after security fixes.
2. Phase 2 (reliability) — retry and crash recovery prevent real data loss.
3. Phase 3 (async DB) — the scalability ceiling won't move without this.

Phases 4-7 can be parallelized if you have help. Phase 1 and Phase 2 are sequential dependencies.

---

## Dependency Graph

```
Phase 0 (security)     <- day 1-2, no deps, ships immediately
    |
Phase 1 (type system)  <- weeks 1-2, depends on Phase 0 for error types
    |
    +-- Phase 2 (ops)  <- weeks 2-3, needs typed errors for retry/observability
    |       |
    |       +-- Phase 3 (perf) <- weeks 3-4, needs event-driven wakeup from 2.3
    |       |
    |       +-- Phase 5 (testing) <- weeks 6-7, needs stable APIs from 1+2+3
    |
    +-- Phase 4 (mobile) <- weeks 4-6, parallel with Phase 3
    |
    +-- Phase 6 (release) <- weeks 7-8, needs all prior phases stable

Phase 7 (polish)       <- weeks 8-10, needs everything else done
```

---

## Quick Wins (Can ship in first 48 hours)

These are the highest-impact changes with the lowest effort:

| What | File | Effort | Impact |
|---|---|---|---|
| Constant-time token comparison | `api.rs`, `mobile_gateway.rs` | 1 hour | Critical security fix |
| Auth token file 0600 | `main.rs:62` | 15 min | Critical security fix |
| Socket path to `~/.richter/` | `main.rs`, `client.rs` | 2 hours | TOCTOU fix + docs alignment |
| File permissions on startup | `main.rs` | 1 hour | Defense in depth |
| Bound MCP channels | `transport.rs` | 30 min | Memory leak prevention |
| Fix `cache_hits_today` reset | `run_manager.rs` | 15 min | Misleading metric fix |
| Bump broadcast capacity to 1024 | `event_bus.rs:13` | 1 line | Headroom for 10+ agents |
