# Performance & Scalability Review — Richter Codebase

**Date:** 2026-05-19
**Reviewer:** Sub-agent 5 (Performance & Scalability)
**Scope:** `richter-core`, `richter-daemon` — all performance-critical paths
**Method:** Line-by-line code review of db.rs, classifier.rs, fingerprint.rs, retry.rs, run_manager.rs, supervisor.rs, scheduler.rs, event_bus.rs, watcher.rs, metrics.rs, rate_limiter.rs, API (mod.rs, handlers.rs, auth.rs, middleware.rs), and benchmarks.

---

## Overall Score: **6.5 / 10**

Richter is well-engineered for its target niche — a local macOS workstation serving 1–10 agents. The codebase shows clear improvement over prior iterations: SQLite now uses sqlx with an 8-connection pool and WAL mode, the scheduler uses `Notify`-driven wakeup instead of busy-polling, process supervision is properly async, the in-memory cache uses LRU eviction, auth tokens are cached in `OnceLock`, watcher uses binary-search prefix matching, and MCP transport channels are bounded. These are genuine, meaningful fixes.

That said, several medium-severity issues remain that would cause tail latency spikes and throughput degradation beyond ~50–100 concurrent agents, and a few high-severity patterns that could bite even at modest load.

---

## 1. Database (SQLite)

### Strengths
- **WAL mode + foreign keys + 5s busy timeout** — correct for single-machine deployment.
- **8-connection pool** via `sqlx::SqlitePoolOptions` — adequate for current scale; `sqlx` transparently caches prepared statements.
- **Transaction-wrapped migrations** with atomic schema version bumps — safe against half-migrated states.
- **Pre-migration backups** — copies the DB file before applying migrations.
- **Integrity check on startup** — catches corruption early.
- **Pagination with limits** (`LIMIT ?2 OFFSET ?3`, capped at 500) — prevents unbounded result sets.
- **Proper indexing** — single-column indexes on most foreign keys and query columns.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **Dynamic SQL in `list_events()`** | `db.rs:290-330` | Medium | Builds `WHERE` clause by concatenating `format!("run_id = ?{param_idx}")` strings. Every distinct filter permutation produces a unique SQL string that can't be reused from sqlx's prepared-statement cache. At scale, this binds memory in sqlx's statement cache for many near-identical queries. |
| **No composite indexes** | Migration v1 | Low | Common query pattern `SELECT * FROM events WHERE run_id = ? ORDER BY created_at DESC` would benefit from a composite `(run_id, created_at DESC)` index. Currently uses two separate single-column indexes. |
| **Sequential cache double-lookup** | `run_manager.rs:275-285` | Medium | On the hot `run_or_join` path, DB cache miss triggers two sequential queries: first by `fingerprint.cache_key()`, then by `fingerprint.cross_worktree_key()`. These could be combined into a single `WHERE fingerprint = ?1 OR fingerprint = ?2` query. |
| **No connection acquire timeout** | `db.rs:33` | Low | Only SQLite's busy_timeout (5s) protects against connection exhaustion. sqlx's `acquire_timeout` is not set, meaning a pool-exhaustion scenario could block indefinitely. |
| **`list_non_expired_cache()` fetches all** | `db.rs:155` | Low | `SELECT * FROM run_cache WHERE expires_at IS NULL OR expires_at > datetime('now')` — with 10,000 cached entries this is a full table scan. Called on startup; fine for current scale but not at 100K+ entries. |

---

## 2. Concurrency

### Strengths
- **`DashMap` for concurrent maps** — sharded, lock-free for reads, appropriate for `active_by_fingerprint`, `active_by_id`, `children`, `completed`.
- **`ParkingMutex` for inner values** — lighter than `std::sync::Mutex`, no poisoning panics, appropriate for `ActiveRun`, `SupervisedChild`, `LruCache`, `watch::Sender`.
- **`Notify`-driven scheduler queue processing** — single task wakes on `release()` instead of per-queued-run polling. Major improvement over prior versions.
- **`watch::channel` for run completion** — event-driven instead of polling; subscribers are notified immediately when a run completes.
- **Proper async process handling** — `kill()` uses `child.start_kill()` + `child.wait().await`, no `block_on` deadlocks.
- **`tokio::spawn_blocking` for git diff** in the completion hook — correct separation of blocking I/O.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **`find_superset()` linear scan** | `run_manager.rs:430-440` | High | Iterates ALL active fingerprints via `self.active_by_fingerprint.iter()` for every `run_or_join` call. At 100 active runs this is O(100) per call; at 1,000 it's O(1,000). This is on the hot path for every command. No auxiliary index exists. |
| **`invalidate_repo_cache()` locks entire LRU** | `run_manager.rs:387-396` | Medium | Acquires `self.cache.lock()`, iterates ALL 10,000 entries, collects matching keys into a `Vec`, then pops them one by one. The lock is held for the entire iteration. Any concurrent cache lookup blocks until iteration completes. |
| **Broadcast capacity 256** | `event_bus.rs:14` | High | `const BROADCAST_CAPACITY: usize = 256`. Under burst load (file changes during `cargo build` can generate hundreds of events per second), slow SSE/MCP subscribers lag and get `RecvError::Lagged`. This is a hard ceiling, not soft backpressure. |
| **`ResourceMonitor::poll()` creates fresh data** | `scheduler.rs:53-63` | Medium | `collect_snapshot()` creates a new `sysinfo::Disks` on every call (not cached). `Disks::new_with_refreshed_list()` iterates all mounted filesystems — several syscalls on macOS. Called via `current()` which re-polls if >5s stale. |
| **Completed run cleanup spawns unbounded** | `supervisor.rs:155-174` | Low | `tokio::spawn` for the periodic cleanup loop runs every 300s, which is fine. But `completed_limit` defaults to 500 entries with no backpressure on eviction speed. |
| **`start_or_reserve` DashMap entry API** | `run_manager.rs:445-535` | Good | Uses `DashMap::entry()` API for atomic check-and-insert. This is correct and avoids the TOCTOU race that existed before. **No issue.** |
| **Stall detection per-run** | `supervisor.rs:276-292` | Low | Each run spawns a `tokio::spawn` loop that wakes every `NO_OUTPUT_TIMEOUT_SECS/2` seconds (150s). This is infrequent and acceptable for typical run counts. |

---

## 3. Memory

### Strengths
- **LRU cache with 10,000 entry cap** — prevents unbounded growth of in-memory results.
- **Output buffer capped at 1MB** per run (`MAX_OUTPUT_BUFFER_BYTES`).
- **Completed runs capped at 500** (`MAX_COMPLETED_RUNS`) with periodic cleanup.
- **Event bus coalescing** — `COALESCE_WINDOW_MS = 250ms` window reduces duplicate event memory.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **No memory-weighted cache eviction** | `run_manager.rs:88` | Medium | `LruCache::new(NonZeroUsize::new(10_000))` — evicts by entry count only. A single 1MB result is treated the same as a 100-byte result. 10,000 × 1MB = theoretical 10GB worst case. |
| **`stream_output()` clones entire output** | `supervisor.rs:253-265` | Medium | `let current = child_clone.output.lock().clone()` — locks the output buffer and clones the entire string (up to 1MB). Multiple SSE subscribers to the same run each get an independent copy. |
| **Unbounded `coalesce` DashMap** | `event_bus.rs:88` | Low | `DashMap<String, CoalescenceState>` — entries are added for every unique event variant but never cleaned up. Over very long uptimes with many distinct event types, this grows slowly but without bound. |
| **Rate limiter `HashMap` only cleaned on explicit `cleanup()`** | `rate_limiter.rs` | Low | `buckets: RwLock<HashMap<String, TokenBucket>>` grows with distinct client IDs. `cleanup()` removes stale entries after 600s idle, but it's never called automatically from middleware — only from an explicit call. |

---

## 4. Caching

### Strengths
- **Two-tier cache** — in-memory LRU (10K entries, instant lookup) + SQLite persistent cache (survives restarts).
- **Cross-worktree cache key** — `fingerprint_cross_worktree` allows dedup across different checkouts of the same repo.
- **Cache freshness with file mtime** — `CachedResult::is_fresh()` checks both TTL and file modification times for invalidation.
- **DB cache eviction** — `evict_expired_cache()` removes entries past `expires_at`.
- **Async freshness check** — `is_fresh()` wrapper uses `spawn_blocking` for filesystem I/O.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **`is_fresh_blocking()` on hot path** | `run_manager.rs:268-270` | Medium | The sync `is_fresh_blocking()` is called from `run_or_join` while holding no lock, which is fine — but it performs `std::fs::metadata` on every file in `changed_files`. Under concurrent load, this is a burst of blocking syscalls on the async runtime thread. The async wrapper `is_fresh()` exists but is not used in the `run_or_join` hot path. |
| **DB cache returns empty output if file missing** | `run_manager.rs:286-298` | Low | If the `output_path` file doesn't exist or can't be read, the cached result returns an empty `String`. No fallback to recomputation. Silent data loss. |
| **No background cache eviction for in-memory LRU** | `run_manager.rs` | Low | Stale entries stay in RAM until they're LRU-evicted by newer entries. There's no periodic TTL cleanup of the in-memory cache. |

---

## 5. File Watching

### Strengths
- **Uses `notify` crate with `RecommendedWatcher`** — on macOS, this uses FSEvents, which is kernel-efficient.
- **Binary-search prefix matching** — `find_repo_for_path()` uses `partition_point` on sorted roots with segment-boundary validation. O(log n) instead of O(n).
- **Event coalescing** — `COALESCE_WINDOW_MS = 250ms` window deduplicates identical `FileChanged` and `ResourcePressure` events.
- **Exclude patterns** — `.git/objects/`, `node_modules/`, `.DS_Store`, swap files excluded from processing.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **`path.canonicalize()` on every event** | `watcher.rs:219` | Medium | `path.canonicalize()` does a `realpath` syscall — expensive during a build that generates thousands of temp files. Should be deferred or skipped for excluded paths. |
| **`should_include()` linear scan of 8 patterns** | `watcher.rs:254-258` | Low | `EXCLUDE_PATTERNS.len() == 8` — negligible at 8 patterns, but the implementation is `for pattern in EXCLUDE_PATTERNS { ... }` doing substring matching per pattern. A compiled globset would be marginally faster. |
| **`changed_paths` uses `Vec::contains()` + `Vec::remove(0)`** | `watcher.rs:226-231` | Medium | `state.changed_paths.contains(&path_str)` is O(n). `state.changed_paths.remove(0)` at cap=500 is O(500) shifting all elements. Should be `HashSet<String>` for dedup + `VecDeque<String>` for ordering. |
| **Event channel capacity 1024** | `watcher.rs:61` | Low | `mpsc::channel(1024)` for raw filesystem events. Under extreme burst (e.g., `cargo clean && cargo build`), this could fill up and block the watcher callback, causing `blocking_send` to block the notify thread. Unlikely in practice. |

---

## 6. API Performance

### Strengths
- **Unix socket transport** — no TCP overhead, localhost-only.
- **Auth token cached in `OnceLock<String>`** — single read at startup, `ConstantTimeEq` comparison via `subtle` crate.
- **Rate limiting via token bucket** — per-client, with `RwLock<HashMap<String, TokenBucket>>` allowing concurrent reads.
- **30s request timeout** via `tower-http::TimeoutLayer`.
- **Scope-based auth** — `read`, `write`, `admin` levels with proper enforcement.
- **CORS restricted to localhost ports** — appropriate for a local daemon.
- **Request ID middleware** — UUID per request for tracing.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **Rate limiter uses single client ID "unix-socket"** | `middleware.rs:26-33` | Medium | All API requests share one token bucket via `state.rate_limiter.check("unix-socket")`. This means a single misbehaving agent can exhaust the global 300 req/min budget for all agents. Per-agent rate limiting would be more appropriate if agent IDs were extractable from auth scope. |
| **`audit_handler` drains broadcast channel** | `handlers.rs:438-480` | Medium | Creates a new `subscribe_all()` receiver and drains it with `try_recv()` in a loop. This creates a snapshot of recent events, but it also means the receiver lags behind real-time for the duration of iteration. If the channel is empty, the endpoint returns immediately with 0 entries — not very useful. |
| **No response size limits on SSE stream** | `handlers.rs:385-407` | Low | `events_handler` creates an SSE stream that runs until the client disconnects or the channel closes. No max-event count or timeout on the SSE stream itself (the 30s tower timeout applies to the initial response, not the long-lived stream). |
| **`health_handler` does a DB query** | `handlers.rs:32-34` | Low | Every `/health` hit runs `db.list_active_runs()`. This is a lightweight query, but under aggressive health-check polling (e.g., 1/second from monitoring), it adds unnecessary DB load. Consider a cached health status updated periodically. |

---

## 7. Scalability Ceiling

### Current Limits

| Metric | Ceiling | Bottleneck |
|--------|---------|------------|
| Concurrent active runs (processes) | 6 global, 3 per-repo | `SchedulerConfig::global_max=6, repo_max=3` |
| Concurrent dev servers | Unbounded | No limit in `SchedulerConfig` or `Supervisor` |
| In-memory cache entries | 10,000 | `LruCache` capacity |
| Completed runs retained | 500 | `MAX_COMPLETED_RUNS` |
| Broadcast capacity | 256 events | `BROADCAST_CAPACITY` |
| API rate limit | 300 req/min global | Single token bucket |
| DB connections | 8 | `SqlitePoolOptions::max_connections(8)` |
| Queue depth | 64 | `SchedulerConfig::queue_max` |

### Agent Scale Estimates

| Agents | Verdict | Reasoning |
|--------|---------|-----------|
| 1–10 | ✅ Runs smoothly | All subsystems well within limits. SQLite, event bus, cache, and scheduler handle this load easily. |
| 50 | ⚠️ Viable with tuning | (1) Increase `BROADCAST_CAPACITY` to 4096; (2) increase SQLite pool to 16–32; (3) replace `find_superset()` linear scan with hashmap index; (4) increase `global_max` and `repo_max`. |
| 100 | ⚠️ Marginal | Same fixes as 50, plus: (5) async `is_fresh()` call on cache path; (6) memory-weighted LRU eviction; (7) per-agent rate limiting; (8) `ResourceMonitor` cache `Arc` optimization. |
| 1,000 | ❌ Not viable | SQLite serialization, single-process event bus, unsharded cache, unbounded dev servers, and `clone()`-heavy event dispatch make this impractical without architectural changes (Redis for cache, PostgreSQL for persistence, event streaming, distributed scheduling). |

### Can it grow without major rework?

**For its intended use case (single workstation, <50 agents):** Yes, with modest tuning (items 1–4 above).

**For multi-machine / enterprise scale:** No. Requires: (a) replacing SQLite with PostgreSQL or sharding across multiple daemon instances, (b) distributed cache (Redis/Memcached), (c) partitioned event bus (Kafka/NATS), and (d) per-agent rate limiting.

---

## 8. Async Patterns

### Strengths
- **All DB operations are async** via sqlx — no blocking DB calls on the async runtime.
- **`tokio::process::Command` for subprocesses** — correct async process management.
- **`tokio::task::spawn_blocking` for git diff** — blocking I/O offloaded to the blocking thread pool.
- **`watch::channel` for completion signaling** — event-driven, no polling.
- **`Notify` for scheduler queue** — efficient wakeup instead of busy-wait.

### Weaknesses

| Issue | Location | Severity | Evidence |
|-------|----------|----------|----------|
| **`std::fs::metadata` in `is_fresh_blocking()` on async path** | `run_manager.rs:268` | Medium | Called from `run_or_join()` which is an async function. Although it doesn't hold a lock during the call (it returns the `CachedResult` clone first), the syscall still blocks the tokio worker. The async `is_fresh()` method exists but isn't used here. |
| **`Path::canonicalize()` in `run_or_join()` validation** | `run_manager.rs:175` | Low | `spec.repo.canonicalize()` is a blocking syscall on each `run_or_join` invocation. Usually fast on local FS, but adds latency on cold caches or network mounts. |
| **`ResourceMonitor::current()` clones snapshot** | `scheduler.rs:71-77` | Low | `self.snapshot.lock().clone()` clones the entire `ResourceSnapshot` struct (CPU, memory, disk stats). Called on every `acquire()`. The struct is small (~40 bytes), so this is negligible. |

---

## 9. Hot Paths

### Where does the system spend most time?

1. **`run_or_join()` → fingerprint computation** — Spawns 4 concurrent git commands + toolchain version lookup. ~50-200ms typically (dominated by git commands). This is the right bottleneck to have — it's doing real work.

2. **`run_or_join()` → in-memory cache lookup** — LRU get is O(1) amortized. Very fast.

3. **`run_or_join()` → DB cache miss query** — Two sequential SQLite queries on cache miss. ~1-5ms each.

4. **`run_or_join()` → `find_superset()`** — Linear scan over active runs. O(n) where n = active concurrent runs.

5. **`supervisor::spawn()` → process creation** — `tokio::process::Command` spawn + output reader tasks. OS-dependent; typically 10-50ms.

6. **`watcher::process_event()` → canonicalize + match** — Per filesystem event. Typically microseconds, but `canonicalize` is a syscall.

7. **`event_bus::emit()` → clone + broadcast** — Event clone is O(event_size). DashMap lookup for coalescence is O(1) amortized. Broadcast send is O(subscribers).

### Classifier Performance
The classifier is not a hot path concern. It's pure CPU with string matching — O(argv length). Benchmark exists (`classifier_bench`) and should be <1μs per classification. ✅

---

## 10. Benchmarking

### Existing Benchmarks
Three criterion benchmark suites exist in `crates/richter-core/benches/`:

| Benchmark | What it measures | Quality |
|-----------|-----------------|---------|
| `classifier_bench` | Classification throughput for 22 toolchain commands + 1000-command stress test | Good — covers hot path, real-world commands, stress scenarios |
| `fingerprint_bench` | BLAKE3 fingerprint timing with git operations + throughput (1000 commands) + micro (no-git) comparison | Good — isolates git I/O from pure hashing |
| `redact_bench` | Secret redaction for various patterns (API keys, private keys, JWTs) + throughput (1KB, 10KB, 100KB) + JSON redaction | Good — covers the output-processing hot path |

### Missing Benchmarks
- **No integration benchmarks** for `run_or_join` end-to-end latency.
- **No scheduler benchmarks** for queue acquisition/release throughput.
- **No DB benchmarks** for cache lookup, run insertion, or event logging.
- **No event bus benchmarks** for emit throughput under subscriber load.
- **No watcher benchmarks** for event processing throughput.

All existing benchmarks are **micro-benchmarks**. They're well-structured and cover the right hot paths, but there's no realistic end-to-end load testing.

---

## 11. Strengths Summary

1. **Correct async architecture** — No `block_on` deadlocks, proper `spawn_blocking` for git/fs, `Notify`-driven scheduling, `watch` channels for completion.
2. **Good concurrency primitives** — `DashMap` for shared maps, `ParkingMutex` for inner values, `LruCache` for bounded cache.
3. **Defense in depth** — Input validation (`validate_command`, `validate_shell_command`, dangerous env key blocking), process group killing, stall detection, output buffering limits, secret redaction.
4. **Well-structured scheduler** — `Semaphore`-based concurrency control with per-repo limits, resource pressure detection, disk-space gate, and `Notify`-based queue processing.
5. **Two-tier caching** — In-memory LRU for speed + SQLite for persistence, with cross-worktree cache keys.
6. **Comprehensive observability** — Event bus with coalescence, Prometheus metrics, structured logging, audit trail, run lifecycle events.
7. **Security-first auth** — `ConstantTimeEq` token comparison, scope-based authorization, restrictive file permissions (0600), shell injection prevention.
8. **Clean separation of concerns** — `RunManager` → `Scheduler` → `Supervisor` layered architecture.

---

## 12. Weaknesses Summary

| # | Weakness | Severity | Category |
|---|---------|----------|----------|
| W1 | `find_superset()` linear scan over all active runs | High | Concurrency |
| W2 | Broadcast capacity too small (256) | High | Scalability |
| W3 | `invalidate_repo_cache()` locks entire LRU for full iteration | Medium | Concurrency |
| W4 | Sequential double DB cache lookup | Medium | Database |
| W5 | Dynamic SQL in `list_events()` defeats prepared-statement caching | Medium | Database |
| W6 | No memory-weighted LRU eviction | Medium | Memory |
| W7 | `is_fresh_blocking()` used on async hot path instead of async version | Medium | Async patterns |
| W8 | `ResourceMonitor::poll()` creates `Disks` on every call | Medium | Hot paths |
| W9 | `changed_paths` uses `Vec::contains()` + `Vec::remove(0)` — O(n) ops | Medium | File watching |
| W10 | `path.canonicalize()` on every filesystem event | Medium | File watching |
| W11 | Rate limiter uses single global client ID | Medium | API |
| W12 | `stream_output()` clones entire 1MB buffer | Medium | Memory |
| W13 | `list_non_expired_cache()` full table scan on startup | Low | Database |
| W14 | Unbounded `coalesce` DashMap in event bus | Low | Memory |
| W15 | No periodic TTL cleanup of in-memory LRU cache | Low | Caching |
| W16 | `should_include()` linear pattern matching | Low | File watching |
| W17 | `health_handler` runs DB query on every request | Low | API |
| W18 | No acquire timeout on SQL connection pool | Low | Database |

---

## 13. Priority Fixes

| Priority | Fix | Effort | Impact |
|----------|-----|--------|--------|
| 🔴 P0 | Increase `BROADCAST_CAPACITY` to 4096 or make configurable | Trivial | Prevents event loss under burst load |
| 🔴 P0 | Replace `find_superset()` linear scan with `HashMap<String, CommandFingerprint>` index | Medium | Eliminates O(n) per `run_or_join` call |
| 🟡 P1 | Use async `is_fresh()` on the `run_or_join` hot path instead of `is_fresh_blocking()` | Low | Removes blocking I/O from cache-hit path |
| 🟡 P1 | Fix `invalidate_repo_cache()` to use `LruCache::retain` or avoid full iteration | Low | Reduces lock hold time on cache invalidation |
| 🟡 P1 | Combine sequential DB cache lookups into single `WHERE fingerprint IN (?, ?)` query | Low | Halves DB round-trips on cache miss |
| 🟡 P1 | Cache `Disks` in `ResourceMonitor` and only refresh disk info every 30s | Low | Eliminates unnecessary disk enumeration |
| 🟡 P1 | Add memory-weighted LRU eviction (cap total cached bytes, not just entry count) | Medium | Prevents memory blowup from large cached outputs |
| 🟡 P1 | Replace `changed_paths` Vec with `HashSet<String>` + `VecDeque<String>` | Low | O(1) contains and O(1) eviction |
| 🟢 P2 | Make `list_events()` use fixed SQL template with null parameters | Low | Improves prepared-statement cache hit rate |
| 🟢 P2 | Add connection acquire timeout to `SqlitePoolOptions` | Trivial | Prevents indefinite blocking on pool exhaustion |
| 🟢 P2 | Add per-agent rate limiting instead of single global bucket | Medium | Prevents one agent from starving others |
| 🟢 P2 | Add integration benchmarks for `run_or_join` E2E latency | Medium | Enables performance regression detection |
| 🟢 P2 | Add periodic TTL cleanup task for in-memory LRU cache | Low | Ensures stale entries don't linger |

---

## 14. Bottom Line

Richter's current performance and scalability profile is **solid for its intended deployment** — a local developer workstation handling 1–10 concurrent agents. The architecture is sound: async runtime, event-driven scheduling, two-tier caching, LRU eviction, proper process supervision, and defense-in-depth security.

The system would **struggle at 100+ concurrent agents** due to the `find_superset()` linear scan, undersized broadcast channel, unweighted cache eviction, and SQLite connection pool limits. These are fixable without architectural changes — indexing, configuration tuning, and async I/O fixes would extend the viable ceiling to ~50–100 agents.

At **1,000+ agents**, fundamental architectural changes would be needed: distributed persistence (PostgreSQL or Citus), distributed cache (Redis), event streaming (NATS/Kafka), and potentially multi-instance daemon deployment. This is outside the current design scope and would constitute a major version change (Richter v2).

**Score breakdown:** Architecture 7/10, Code quality 8/10, Correctness 8/10, Scalability 5/10, Observability 7/10, Benchmarking 5/10 → **Overall: 6.5/10**.
