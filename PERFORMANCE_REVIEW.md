# Richter Performance & Scalability Review

**Scope:** `richter-core`, `richter-daemon`, `richter-mcp`, `richter-cli`
**Date:** 2026-05-17
**Reviewer:** Sub-agent 5 (Performance & Scalability)

---

## Executive Summary

Richter is architecturally sound for a single-workstation, single-daemon control plane, but it has several **O(n) hot paths**, **synchronous I/O in async contexts**, and **SQLite connection limits** that will throttle throughput well before 1,000 concurrent agents. The codebase uses `DashMap` and `ParkingMutex` correctly in most places, but cache invalidation, superset scanning, and event-bus cloning create non-trivial overheads under load.

**Verdict:**
- **10 concurrent agents:** ✅ Fine with minor tuning.
- **100 concurrent agents:** ⚠️ Viable on a beefy machine, but event-bus lag, SQLite serialization, and linear scan hot paths will cause tail latency spikes.
- **1,000 concurrent agents:** ❌ Not viable without architectural changes (connection sharding, event-bus backpressure, cache sharding, and removal of blocking I/O from async tasks).

---

## 1. SQLite Query Patterns

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **Dynamic SQL string building** (`format!` + `String::push_str`) prevents prepared-statement reuse | `db.rs`, `list_events()` | Medium |
| **No composite index** on `(events.run_id, events.created_at)` — common query pattern | `db.rs`, migration v1 | Low |
| **Pool limited to 8 connections** | `db.rs`, `SqlitePoolOptions::new().max_connections(8)` | Medium |
| **No connection/acquire timeout configured** beyond 5s busy_timeout on the pragma | `db.rs` | Low |
| **Cache lookup does two sequential queries** (hash → cross-worktree hash) | `run_manager.rs` | Medium |

### Details

- **`list_events`** builds a SQL string by concatenating `Vec<String>` clauses and formatting index placeholders (`?{param_idx}`). This defeats SQLx's prepared-statement cache for every distinct filter permutation.
- **Cache lookup in `run_manager.rs`** first queries by `fingerprint.cache_key()`, and if that misses, queries again by `cross_worktree_hash`. For a cache miss (the common case when a run is genuinely new), this is **two round-trips**.
- **Index audit:** The schema has single-column indexes on `runs(repo_id)`, `events(run_id)`, etc. A composite index `(events(run_id, created_at DESC))` would speed up the common "events for a run, newest first" query.

### N+1 Queries

No classic N+1 query patterns were found. The code generally fetches related data in single queries or uses `fetch_all`. However, the **sequential double cache lookup** is effectively an N+1 for the cache path.

---

## 2. Fingerprinting / Classification Performance

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **Toolchain version cache uses `std::sync::Mutex`, not async-aware** | `fingerprint.rs` | Low |
| **`to_string()` + `to_vec()` allocations** for every toolchain version fetch | `fingerprint.rs` | Low |
| **`std::fs::read` is synchronous** inside an async fingerprint function | `fingerprint.rs`, `hash_git_state` | Low |
| **Classifier allocates a new `Vec` and re-strings every arg** | `classifier.rs` | Low |
| **No regex cache / compiled patterns** but classifier doesn't use regex (good) | `classifier.rs` | N/A |

### Details

- Fingerprinting runs git commands concurrently via `tokio::join!` — this is correct and fast.
- The toolchain cache hit path is a single `Mutex` lock; at high concurrency this may cause micro-contention but the lock is held for microseconds.
- `std::fs::read` for lockfiles is fine for local SSDs but blocks the async runtime thread. Under heavy load (1,000s of fingerprints/sec), this would stall the executor.

---

## 3. Event Bus Throughput

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **Broadcast capacity is only 256** | `event_bus.rs`, `BROADCAST_CAPACITY` | High |
| **Every `emit()` clones the event twice** (`event.clone()` for coalescence, then `send`) | `event_bus.rs`, `emit()` | Medium |
| **Coalescence map is unbounded** (`DashMap<String, CoalescenceState>`) | `event_bus.rs` | Low |
| **`FilteredReceiver` drops events in a tight `loop` on `try_recv`** | `event_bus.rs` | Low |

### Details

- With 256 broadcast capacity, any consumer that falls behind (e.g., a slow SSE client, a lagging MCP subscriber) will be **dropped with `RecvError::Lagged(n)`**.
- At 100 concurrent agents each consuming events, and a burst of file-change events from a large `cargo build`, the 256-slot buffer will overflow.
- The event is cloned at least twice per emit: once into the DashMap coalescence state, once into the broadcast channel.

---

## 4. Concurrent `run_or_join` Handling

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **`find_superset()` does a linear scan over ALL active runs** | `run_manager.rs` | High |
| **`invalidate_repo_cache()` iterates ALL cache entries linearly** | `run_manager.rs` | High |
| **`active_by_fingerprint` key is a full `CommandFingerprint` struct with many `String` fields** | `run_manager.rs` | Medium |
| **`is_destructive()` and `is_dev_server()` allocate `to_lowercase()` on every call** | `run_manager.rs` | Low |
| **Cache uses a single `ParkingMutex<LruCache>`, no sharding** | `run_manager.rs` | Medium |

### Details

- **`find_superset()`**: `for entry in self.active_by_fingerprint.iter()` — this is O(active_runs). At 100 active runs it's fine; at 1,000 it becomes a bottleneck, especially because each `ActiveRun` is protected by a `ParkingMutex` and the loop doesn't actually need to lock them, but the iterator still traverses the whole DashMap.
- **`invalidate_repo_cache()`**: Iterates the entire LRU cache (up to 10,000 entries), locking the cache for the entire duration. This is called on file-system changes.
- **Cache contention**: All cache access (hits, inserts, invalidations) serializes through a single `ParkingMutex`. Even `get()` requires locking the entire LRU.
- **String-heavy keys**: `CommandFingerprint` contains three `String` fields. DashMap hashing is fast, but cloning the key on insert is not free.

---

## 5. Process Output Capture (Piping / Buffering)

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **Output buffer is capped at 1MB per run** — scales linearly with active runs | `supervisor.rs` | Medium |
| **`stream_output()` clones the entire output `String` into a channel** | `supervisor.rs` | Medium |
| **Completion task shells out to `git diff --name-only HEAD` using `std::process::Command` (blocking)** | `run_manager.rs`, `start_new()` | High |
| **Stall detection spawns one sleeping task per active run** | `supervisor.rs` | Low |
| **`wait_for_completion()` polls in a loop with 100ms timeout** (not purely event-driven) | `supervisor.rs` | Low |

### Details

- 1MB × 1,000 active runs = **1GB of buffered output in memory**. The default scheduler limits to 6 global concurrent runs, but dev servers are unbounded. If many dev servers run simultaneously, memory pressure is real.
- Every call to `stream_output()` clones the full 1MB string. If multiple SSE clients stream the same run, each gets an independent clone.
- The completion hook in `start_new()` calls `std::process::Command::new("git")...output()` inside a `tokio::spawn` async block. This blocks the async worker thread for the duration of the git command. Under load, this starves the runtime.

---

## 6. File Watcher Efficiency (FSEvents)

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **`RecursiveMode::Recursive` on every repo root** — can be expensive for large repos | `watcher.rs` | Medium |
| **`path.canonicalize()` called for every single event path** | `watcher.rs` | Medium |
| **`should_include()` does linear scan of 8 exclude patterns per path** | `watcher.rs` | Low |
| **`changed_paths` uses `Vec::contains()` (linear) and `remove(0)` (O(n) shift)** | `watcher.rs` | Medium |
| **Binary search in `find_repo_for_path` is good, but locks `sorted_roots`** | `watcher.rs` | Low |

### Details

- `notify` on macOS uses FSEvents, which is efficient at the kernel level, but the user-space callback (`process_event`) does heavy string manipulation.
- `canonicalize()` on macOS involves `realpath` syscalls; during a large build (thousands of temporary files), this is non-trivial.
- `changed_paths` caps at 500 entries, but `remove(0)` shifts 499 elements on every eviction. A `VecDeque` or `HashSet` + `Vec` would be better.

---

## 7. Memory Usage of Cached Outputs

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **In-memory cache stores full `String` output** (up to 1MB per entry, 10,000 entries max) | `run_manager.rs` | Medium |
| **Persistent DB cache stores `output_path` only, not the output itself** | `db.rs` | Good |
| **No compressed in-memory storage** | `run_manager.rs` | Low |
| **DB cache read uses `String::from_utf8` on full file contents** | `run_manager.rs` | Low |

### Details

- In-memory LRU: 10,000 entries × 1MB = **theoretical 10GB max**. In practice, most outputs are smaller, but there's no per-entry size cap or eviction based on memory pressure.
- The DB cache path reads the entire file from disk and decompresses it (if gzipped). For large logs (10MB+), this spikes memory temporarily.

---

## 8. Cache Eviction Strategy

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **In-memory: LRU with fixed 10,000 entry cap** | `run_manager.rs` | Good |
| **No memory-pressure-aware eviction** (only entry-count cap) | `run_manager.rs` | Medium |
| **DB: time-based eviction (`expires_at < now`)** | `db.rs` | Good |
| **`evict_expired_cache` only evicts DB entries, not memory cache** | `db.rs` | Low |

### Details

- The LRU cache correctly evicts least-recently-used entries at 10,000. However, a single 1MB entry is treated the same as a 100-byte entry. A **weighted LRU** (by byte size) would prevent memory blowups from large test outputs.
- Background eviction of in-memory cache is not implemented; stale entries stay in RAM until they fall out of the LRU.

---

## 9. Resource Monitor CPU Overhead

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **`ResourceMonitor::poll()` creates `System::new_all()` every time** | `scheduler.rs` | High |
| **Polls all processes, disks, and memory on every call** | `scheduler.rs` | Medium |
| **No incremental refresh — always `refresh_all()`** | `scheduler.rs` | High |
| **`current()` clones the entire snapshot on every call** | `scheduler.rs` | Medium |

### Details

- `System::new_all()` followed by `refresh_all()` is one of the most expensive operations in `sysinfo`. It iterates all processes and reads `/proc` (Linux) or Mach APIs (macOS). Doing this on **every cache-miss poll** (every 5 seconds) is fine for a workstation, but at 1,000 agents requesting resources simultaneously, it becomes a CPU hog.
- The snapshot is cloned via `self.snapshot.lock().clone()` in `current()`. A shared `Arc` reference would eliminate this copy.

---

## 10. MCP Server Connection Handling

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **Stdio transport spawns blocking threads for stdin/stdout** (acceptable) | `transport.rs` | Good |
| **In-process transport uses unbounded-ish `mpsc::channel(1024)`** | `transport.rs` | Low |
| **MCP server builds tool/resource lists fresh on every request** | `server/protocol.rs` | Low |
| **`McpServer::new()` does I/O (`is_reachable()` on Unix socket) in constructor** | `server/protocol.rs` | Medium |
| **No backpressure on MCP `tools/call` — handlers can block the single message loop** | `server/protocol.rs` | Medium |

### Details

- The MCP message loop is single-threaded per transport: `while let Some(message) = transport.recv().await`. A long-running tool call (e.g., `run_or_join` which can wait minutes) **blocks the entire MCP message loop** for that transport. Other tools/resources cannot be served concurrently on the same connection.
- `StdioTransport` correctly uses `spawn_blocking` for stdin/stdout, which is the right pattern.

---

## 11. Mobile Gateway

### Findings

| Issue | Location | Severity |
|-------|----------|----------|
| **Nonce tracker `check_and_insert()` is O(n) over 10,000 entries** | `mobile_gateway.rs` | Medium |
| **Daily rotation drops 10,000 strings at once** (allocation spike) | `mobile_gateway.rs` | Low |
| **Token-bucket rate limiting not visible in sampled code** | — | N/A |

---

## 12. Concurrency Issues & Risks

| Risk | Impact | Likelihood |
|------|--------|------------|
| **Blocking `git diff` in async completion hook** | Runtime thread starvation under load | High |
| **`find_superset()` linear scan under high concurrency** | Tail latency spikes for run_or_join | Medium |
| **Event bus broadcast overflow (capacity 256)** | SSE clients drop, audit log misses events | Medium |
| **SQLite write serialization with 8 connections** | Queue saturation for metadata writes | Medium |
| **Cache invalidation locks entire LRU** | File-watcher events stall cache reads | Medium |
| **`System::new_all()` CPU spikes** | Scheduler decisions slow down | Low |

---

## 13. Scalability Ceiling Estimates

| Metric | Current Ceiling | Bottleneck |
|--------|-----------------|------------|
| **Concurrent active runs (processes)** | 6 global (default) | `SchedulerConfig::global_max` |
| **Concurrent dev servers** | Unbounded | No limit in supervisor |
| **Agents making requests** | ~50/s sustained | SQLite 8-connection pool |
| **Event throughput** | ~1,000 events/sec | Broadcast clone + DashMap coalescence |
| **Cache size (memory)** | ~10,000 entries (~1–10GB) | LRU count cap only |
| **File-system watch repos** | ~10–50 | FSEvents + `canonicalize()` overhead |
| **`run_or_join` latency (p99)** | <50ms @ 10 agents, >500ms @ 100 agents | `find_superset()` + DB cache miss |

### Can it scale to N concurrent agents?

| N | Verdict | Why |
|---|---------|-----|
| 10 | ✅ Yes | Well within SQLite, broadcast, and memory limits. |
| 100 | ⚠️ Probably | Requires: (1) increase SQLite pool to 32+ and shard DB reads; (2) increase broadcast capacity to 4096+; (3) replace `find_superset()` with an indexed structure; (4) make `git diff` async or offload to blocking pool. |
| 1,000 | ❌ No | SQLite serialization, single-threaded MCP loops, unbounded memory from output buffers, and O(n) scans will collapse. Needs: multi-process sharding, Redis/Memcached for cache, dedicated task pool for blocking I/O, and event bus partitioning. |

---

## 14. Recommended Fixes (Priority Order)

1. **🔴 Replace blocking `git diff` in completion hook**
   - Use `tokio::task::spawn_blocking` or a dedicated blocking thread pool for `std::process::Command` in `run_manager.rs:start_new()`.

2. **🔴 Fix `find_superset()` and `invalidate_repo_cache()` linear scans**
   - Maintain an auxiliary `HashMap<ResourceClass, HashSet<CommandFingerprint>>` or similar index.
   - Use `LruCache::retain` (if available in the `lru` version) or switch to a `dashmap`-based sharded cache.

3. **🔴 Increase event-bus broadcast capacity**
   - Change `BROADCAST_CAPACITY` from 256 to at least 4096, or make it configurable.

4. **🟡 Optimize `ResourceMonitor`**
   - Create `System` once and use `refresh_specifics()` instead of `new_all()` + `refresh_all()`.
   - Store the snapshot behind an `Arc` to avoid cloning on read.

5. **🟡 Use prepared-statement-friendly SQL in `list_events()`**
   - Use a fixed-parameter query with `COALESCE` instead of dynamic string concatenation.

6. **🟡 Add memory-weighted eviction to the in-memory cache**
   - Track total cached bytes and evict by size, not just entry count.

7. **🟡 Cache lowercase results for `is_destructive()` / `is_dev_server()`**
   - Precompute a `HashSet` of destructive patterns or use `contains` on `&str` without allocation.

8. **🟡 Fix watcher `changed_paths` data structure**
   - Replace `Vec` + `contains` + `remove(0)` with `HashSet<String>` + `VecDeque<String>` for O(1) ops.

9. **🟢 Make MCP message loop concurrent per connection**
   - Spawn tool/resource handlers in `tokio::spawn` so the recv loop isn't blocked.

10. **🟢 Consider sqlx connection pool increase**
    - Bump `max_connections` from 8 to `num_cpus * 2` for IO-bound reads.

---

## 15. Honest Bottom Line

Richter performs well as a **personal workstation deduplicator** (≈1–10 agents). The architecture is pragmatic for that use case. Past 100 agents, the combination of SQLite serialization, single-node event bus, unsharded in-memory cache, and blocking git calls creates a compounding bottleneck. It will not scale to 1,000 concurrent agents without replacing the persistence layer (or sharding across multiple daemon instances) and reworking the cache and event-dispatch layers.
