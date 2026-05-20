# Performance & Scalability Review — Richter Codebase

**Date:** 2026-05-05
**Scope:** 13 performance-critical source files across `richter-core`, `richter-daemon`, and `richter-mcp`
**Method:** Manual code review + `rg` anti-pattern sweep + architectural bottleneck analysis

---

## 1. Executive Summary

| Metric | Grade / Value |
|--------|---------------|
| **Overall Performance/Scalability Grade** | **5 / 10** |
| **Most Likely Bottleneck** | Single SQLite connection under `parking_lot::Mutex` + scheduler busy-wait loops |
| **Scalability Ceiling** | ~6–10 concurrent heavy agents before DB contention and scheduler task explosion dominate |
| **Can it grow without major rework?** | **No.** Requires async DB, bounded MCP channels, and scheduler refactoring to scale beyond local macOS desktop use. |

---

## 2. Anti-Pattern Sweep (Quantitative)

| Pattern | Count | Risk |
|---------|-------|------|
| `.clone()` | 213 | Moderate — many are cheap (`String`, `Arc`), but hot-path clones add up |
| `Vec::new()` | 40 | Low — mostly small, short-lived vectors |
| `Mutex` / `RwLock` | 72 | **Moderate** — many are `parking_lot` (fast), but several wrap long blocking ops |
| `tokio::spawn` | 33 | **Moderate** — several are per-run or per-queued-run polling loops |
| `mpsc::unbounded_channel` | 4 | **High** — MCP transport; memory growth unbounded under load |
| `mpsc::channel` bounded | 3 | Low — buffers are 1, 64, and 1024; adequate for current loads |

---

## 3. File-by-File Findings

### 3.1 `crates/richter-core/src/fingerprint.rs`
- **Bottleneck:** Every fingerprint call spawns **6 blocking `std::process::Command` invocations** (`git rev-parse`, `git diff --cached`, `git diff HEAD`, `git diff-index`, plus tool version checks) and reads the lockfile from disk synchronously.
- **Impact:** Under concurrent agent load, git commands contend for the same repo’s `.git/index` lock. Fingerprints are on the critical path for every `run_or_join` decision.
- **Fix:** Cache git state with an invalidation window; move fingerprinting to a dedicated blocking thread pool or make it async with `tokio::process`.

### 3.2 `crates/richter-core/src/db.rs`
- **Bottleneck:** **Single SQLite connection** wrapped in `parking_lot::Mutex`. No connection pool, no prepared-statement cache.
- **Impact:** All reads and writes serialize. `list_events` dynamically builds SQL and boxes every parameter as `Box<dyn ToSql>`, allocating per query. The cache hit path in `run_manager` blocks the async executor while holding the DB lock.
- **Fix:** Switch to `sqlx` (already in workspace `Cargo.toml` but unused) with a connection pool, or shard tables. At minimum, cache prepared statements and use `r2d2`/`deadpool` for SQLite.

### 3.3 `crates/richter-core/src/classifier.rs`
- **Verdict:** ✅ Fast. Pure CPU, O(argv) string matching, no IO. Not a bottleneck.

### 3.4 `crates/richter-core/src/resource.rs`
- **Verdict:** ✅ Acceptable for current scale. In-memory priority queue (`VecDeque` with O(n) insert) and `parking_lot::Mutex`. Limits are conservative (`max_heavy_global=3`).
- **Risk:** `ResourceManager` clones the entire queued-run list on every `queued_runs()` call.

### 3.5 `crates/richter-daemon/src/scheduler.rs`
- **Critical Issue:** `acquire()` spawns a **background `tokio::spawn` task for every queued run** that busy-loops every **100 ms** polling `try_acquire_owned()` on semaphores and `is_under_pressure()`.
- **Impact:** With `queue_max=64`, 64 queued runs = 64 tasks waking up 10×/second = **640 wake-ups/sec** doing no useful work. This wastes CPU and scheduler time.
- **Additional Issue:** `ResourceMonitor::poll()` calls `System::new_all()` on every poll (every 5 s or on demand). `sysinfo`’s `new_all()` rescans all processes and memory stats — expensive and unnecessary; use `System::new()` + targeted refreshes.
- **Fix:** Replace polling loops with `Notify`-based wakeup when permits are released or pressure drops.

### 3.6 `crates/richter-daemon/src/run_manager.rs`
- **Critical Issue:** `start_new()` spawns a completion hook that **polls `supervisor.exit_code()` every 200 ms** in a `loop { sleep(200ms) }`.
- **Impact:** Each active run adds another busy-wait task. 10 runs = 50 polls/sec; 100 runs = 500 polls/sec.
- **Additional Issues:**
  - DB cache lookup (`db.get_cache_entry`) is synchronous blocking IO inside async `run_or_join`.
  - `CachedResult::is_fresh()` does `std::fs::metadata` on every file in `changed_files` on every cache lookup — blocking and repetitive.
  - In-memory `DashMap` cache has **no TTL, no eviction, no size cap**. Will grow unbounded for long-lived daemon processes.
- **Fix:** Use `tokio::sync::Notify` or `watch` channels for run completion. Replace fs-stat freshness with a timestamp-based invalidation or async file watcher.

### 3.7 `crates/richter-daemon/src/api.rs`
- **Bottleneck:** `auth_middleware` calls `load_auth_token()` → `std::fs::read_to_string` on **every request**. The token is static; should be cached in `DaemonState`.
- **Bottleneck:** `audit_handler` creates a new broadcast subscriber and drains the event bus in a loop for **every HTTP request** — O(events) work per request.
- **Verdict:** Axum + tower-http is solid. CORS layer is permissive (`Any` origin) — not performance but security note.

### 3.8 `crates/richter-daemon/src/event_bus.rs`
- **Bottleneck:** `tokio::broadcast` capacity is **256**. Under burst loads (e.g., many file changes), slow consumers are dropped with `RecvError::Lagged`. Event coalescing helps, but 256 is tight for >10 agents.
- **Verdict:** Functional but undersized for high-throughput scenarios. Consider `1024` or event-sampling for high-frequency variants like `FileChanged`.

### 3.9 `crates/richter-daemon/src/importance/pipeline.rs`
- **Verdict:** ✅ Safe in default config. LLM boosts are disabled (`use_cheap_model: false`, `use_frontier_model: false`).
- **Risk:** If enabled, the 30-calls/minute circuit breaker in `api.rs` is the only backpressure. Parser list is small (10 parsers) and runs sequentially — acceptable for stdout sizes <1 MB.

### 3.10 `crates/richter-daemon/src/watcher.rs`
- **Bug + Performance:** `find_repo_for_path()` iterates `active_roots`, then calls `self.repo_states.iter().next()` which returns the **first repo in the DashMap**, not the repo matching the root. This means events for repo B may be mis-attributed to repo A.
- **Performance:** `process_event()` calls `path.canonicalize()` (blocking syscalls) on every watch event. `should_include()` does O(n) string-contains against `EXCLUDE_PATTERNS` for every path.
- **Fix:** Pre-compile exclude patterns (e.g., `globset`), build a root→repo lookup map, and do canonicalization only when necessary.

### 3.11 `crates/richter-daemon/src/supervisor.rs`
- **Critical Issue:** `SupervisedChild::kill()` calls `tokio::runtime::Handle::current().block_on(child.wait())` inside a **synchronous method**. If `kill()` is called from an async context (and it is — via `cancel_run()` and orphan reconciliation), this **blocks the async thread** and can cause deadlocks or panics depending on Tokio flavor.
- **Performance:** `stream_output()` clones the entire `MAX_OUTPUT_BUFFER_BYTES` (1 MB) buffer into a new channel for every streaming request.
- **Verdict:** Spawning and output-reading logic is sound, but the `block_on` in `kill()` is a serious concurrency hazard.

### 3.12 `crates/richter-daemon/src/plugin_runtime.rs`
- **Verdict:** Not a runtime bottleneck — plugin discovery runs once at startup. However, it does blocking `std::fs::read_dir` + `Command::output` synchronously.

### 3.13 `crates/richter-mcp/src/server.rs` & `protocol.rs`
- **Bottleneck:** `transport.rs` uses `mpsc::unbounded_channel` for **all** MCP message traffic (4 instances). Under load from an aggressive MCP client, memory grows without bound.
- **Concurrency:** MCP server processes messages **sequentially** per transport connection. No parallelism within a single client session.
- **Verdict:** Fine for a single IDE client (Claude Desktop, Codex). Will not scale to multiple high-frequency MCP peers.

---

## 4. Bottleneck Assessment

### 4.1 Likely Bottlenecks Under Concurrent Agent Load

1. **SQLite DB Mutex** — Every cache lookup, run insert, and event log blocks on one mutex. With >5 agents issuing commands rapidly, DB latency will dominate.
2. **Scheduler Busy-Wait Tasks** — Each queued run consumes a tokio task polling every 100 ms. At queue capacity (64), this is pure overhead.
3. **Git Fingerprinting** — 6 synchronous git commands per dedup check. Git locks serialize access to the same repo.
4. **Run Manager Polling** — 200 ms polling loop per active run for completion detection.
5. **MCP Unbounded Channels** — Memory risk if MCP clients flood the daemon.

### 4.2 Database Query Efficiency & Connection Pooling
- **Grade: D.** Single connection, no pooling, no prepared statement cache, dynamic SQL generation with per-parameter allocations. `sqlx` is in the workspace but never used.

### 4.3 Memory Usage Patterns
- **Leaks:** In-memory `DashMap` run cache in `run_manager` has no eviction — definite leak for long uptimes.
- **Excessive Allocation:** `list_events` boxes every query parameter. Fingerprinting allocates multiple `String` outputs from git commands.
- **Good Limits:** Output buffer capped at 1 MB/run. Watcher `changed_paths` capped at 500.

### 4.4 Caching Strategy Effectiveness
- **Two-tier cache:** In-memory `DashMap` + persistent SQLite `run_cache`.
- **Strengths:** Cross-worktree key sharing is clever.
- **Weaknesses:** No eviction, no size limit, freshness checked via blocking `fs::metadata` on every lookup, DB cache always returns empty output (`String::new()`).

### 4.5 Async Runtime Configuration
- **Default Tokio runtime** (`worker_threads = num_cpus`, `max_blocking_threads = 512`).
- **Risk:** Many blocking operations (SQLite, git, fs, `block_on`) consume blocking threads. Under load, the default 512 blocking threads may be exhausted, causing starvation.

### 4.6 LLM Pipeline Latency Impact on UX
- **Currently zero** in default config (both model boosts disabled). Parsers are fast (<1 ms for typical output). If enabled, 30 calls/min circuit breaker is coarse but acceptable for a local tool.

### 4.7 Filesystem Watch Scalability
- **Buggy root→repo resolution** makes it unreliable for multiple repos. `canonicalize()` and string-matching per event are inefficient. `notify` with FSEvents is the right backend for macOS, but the wrapping logic needs work.

### 4.8 Resource Scheduling Fairness
- **Fair enough for desktop use.** Per-repo limits (max 3) and global limits (max 6) prevent any one repo from monopolizing. Priority queue is basic (single `u8` field). No starvation prevention for low-priority runs.

### 4.9 Concurrency Model & Deadlock Risks
- **Hazard:** `supervisor.kill()` uses `block_on` inside a sync method called from async contexts — this is the single most dangerous concurrency pattern in the codebase.
- **Layering:** `DashMap` + `ParkingMutex` inside values is generally safe because the lock order is consistent (outer DashMap first, inner mutex second).
- **No deadlocks observed** in the DB mutex path because it’s a single coarse lock.

### 4.10 Scalability Ceiling (Agents Before Breakdown)
- **Heavy agents (builds/tests):** ~6 concurrent runs hard-limited by scheduler config.
- **Light agents (lints):** ~12–15 concurrent before SQLite contention causes noticeable latency.
- **Total agents issuing commands:** ~10–20 before the combination of DB mutex, git fingerprinting, and scheduler polling loops degrades UX (>500 ms per `run_or_join`).

### 4.11 Network Efficiency (MCP, Mobile Gateway)
- **MCP:** Unbounded channels are a memory liability. Sequential processing per connection means throughput is limited to ~message latency.
- **Mobile Gateway:** TCP on port 9777, separate from Unix socket. Code not reviewed in depth for this report, but the same `DaemonState` (with its `ParkingMutex` fields) is shared — mobile requests will contend for the same locks.

---

## 5. Recommendations (Prioritized)

| Priority | Action | Effort | Impact |
|----------|--------|--------|--------|
| **P0** | Replace `supervisor.kill()` `block_on` with pure async `child.kill().await` | Low | Eliminates deadlock/crash risk |
| **P0** | Add bounded channels to MCP transport (replace `unbounded_channel`) | Low | Prevents unbounded memory growth |
| **P1** | Switch DB to `sqlx` with a `deadpool`/`sqlx` connection pool | Medium | Removes single-writer bottleneck |
| **P1** | Make scheduler queue use `Notify` instead of per-run polling tasks | Medium | Cuts useless CPU wake-ups |
| **P1** | Cache auth token in `DaemonState` instead of reading disk per request | Low | Reduces pointless syscalls |
| **P2** | Add LRU eviction + TTL to in-memory `DashMap` run cache | Low | Fixes memory leak |
| **P2** | Make fingerprinting async and cache git state (HEAD, dirty, diff) | Medium | Removes git command storm |
| **P2** | Fix `watcher.find_repo_for_path()` to actually match the correct repo | Low | Fixes cross-repo event misrouting |
| **P3** | Increase broadcast channel capacity from 256 to 1024 | Low | Reduces lagged consumers |
| **P3** | Replace `CachedResult::is_fresh()` fs-stat loop with async watcher or timestamp heuristic | Low | Removes blocking IO on cache path |

---

## 6. Conclusion

Richter is **well-architected for a local macOS developer tool serving 1–3 agents**, but it is **not production-scalable** as written. The single SQLite connection, scheduler busy-wait loops, and unbounded MCP channels are architectural ceilings, not mere tuning issues. The good news: the fixes are well-understood (connection pooling, `Notify`-based waiting, bounded channels) and the codebase uses modern Rust patterns (`DashMap`, `tokio`, `axum`) that make refactoring tractable. Without the P0/P1 changes above, expect degradation beyond ~10 agents.
