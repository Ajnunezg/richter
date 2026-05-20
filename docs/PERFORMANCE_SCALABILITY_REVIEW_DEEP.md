# Performance & Scalability Deep Review — Richter Codebase

**Date:** 2026-05-18
**Scope:** Full codebase audit — `richter-core`, `richter-daemon`, `richter-mcp`
**Method:** Line-by-line review of all daemon, database, fingerprint, watcher, MCP, and mobile gateway source. Comparison against existing review to find corrections, gaps, and new issues.
**Purpose:** Go DEEPER than the existing `docs/PERFORMANCE_SCALABILITY_REVIEW.md`. Find what it missed, correct what it got wrong, and surface issues invisible to anti-pattern sweeps.

---

## 0. Corrections to Existing Review

The existing review (`docs/PERFORMANCE_SCALABILITY_REVIEW.md`, dated 2026-05-05) contains several **outdated findings** that reflect an older codebase version. The codebase has been significantly refactored since then.

| Existing Claim | Current Reality | Status |
|---|---|---|
| "Single SQLite connection wrapped in `parking_lot::Mutex`" | `sqlx::SqlitePool` with `max_connections=8`, WAL mode, `busy_timeout=5s` | **CORRECTED** |
| "No connection pool, no prepared-statement cache" | Connection pool exists; `sqlx` caches prepared statements transparently | **CORRECTED** |
| "`list_events` boxes every parameter as `Box<dyn ToSql>`" | Uses `sqlx` typed binds, no dynamic boxing | **CORRECTED** |
| "Scheduler spawns busy-wait task every 100ms per queued run" | Uses single `Notify`-driven queue processor task that sleeps until woken | **CORRECTED** |
| "`supervisor.kill()` uses `block_on` — deadlock risk" | Now properly async: `child.start_kill()` + `child.wait().await` | **CORRECTED** |
| "`auth_middleware` reads token from disk on every request" | Cached in `Arc<OnceLock<String>>` at startup | **CORRECTED** |
| "In-memory `DashMap` cache has no TTL, no eviction, no size cap" | Replaced with `LruCache<CommandFingerprint, CachedResult>` (capacity 10,000) | **CORRECTED** |
| "`watcher.find_repo_for_path()` returns first DashMap entry" | Now uses binary-search prefix matching with segment-boundary validation | **CORRECTED** |
| "MCP transport uses `mpsc::unbounded_channel` (4 instances)" | All channels are bounded (`channel(1024)`, `channel(256)`) | **CORRECTED** |

**Bottom line:** The most severe findings from the old review (single DB mutex, block_on deadlock, scheduler busy-wait, unbounded MCP channels, no cache eviction) have all been fixed. The remaining issues require deeper analysis.

---

## 1. Async Runtime Misuse (Blocking in Async Context)

### 1.1 `CachedResult::is_fresh()` — Blocking `fs::metadata` in async path

**Location:** `run_manager.rs:126-139`

```rust
for file_path in &self.changed_files {
    if let Ok(meta) = std::fs::metadata(file_path) {
        if let Ok(mtime) = meta.modified() { ... }
    }
}
```

This is called from the hot path of `run_or_join()` (line 268). Each `std::fs::metadata` is a blocking syscall. With `changed_files` potentially containing dozens of paths (from `git diff --name-only HEAD`), this is a burst of blocking I/O that stalls the tokio worker thread.

**Impact:** Medium-high. On the critical cache-hit path. Under concurrent load, this creates head-of-line blocking.

**Fix:** Use `tokio::fs::metadata` or move the entire freshness check to `tokio::task::spawn_blocking`. Better yet, replace with a timestamp-based heuristic (cache TTL alone) or an inotify-based file watcher for invalidation.

### 1.2 `run_or_join()` — `Path::canonicalize()` is a blocking syscall

**Location:** `run_manager.rs:160`

```rust
let canonical_repo = match spec.repo.canonicalize() {
```

`canonicalize()` resolves symlinks and normalizes the path — a syscall that can block on network filesystems. Called on every `run_or_join` entry.

**Impact:** Low on local macOS. Medium if repos are on NFS/SMB mounts.

**Fix:** Cache canonicalized paths or use `tokio::fs::canonicalize`.

### 1.3 Completion hook — Synchronous `std::process::Command` for git

**Location:** `run_manager.rs:647-654`

```rust
let changed: Vec<String> = std::process::Command::new("git")
    .args(["diff", "--name-only", "HEAD"])
    .current_dir(&repo_for_cache)
    .output()
    ...
```

Inside a `tokio::spawn` task, using `std::process::Command::output()` which is **synchronous blocking**. This blocks the tokio worker thread while waiting for git to complete. Should use `tokio::process::Command`.

**Impact:** Medium. Each run completion blocks a worker thread for the duration of a `git diff` call (typically 10-500ms depending on repo size).

### 1.4 Completion hook — `std::fs::read()` for cached output

**Location:** `run_manager.rs:425-438`

```rust
let cached_output = match &entry.output_path {
    Some(path) if std::path::Path::new(path).exists() => {
        match std::fs::read(path) { ... }
```

Blocking file read + gzip decompression on the async thread. For large outputs (up to 1MB buffer), this can take noticeable time.

**Impact:** Low-medium. Only on DB cache miss → DB cache hit path.

### 1.5 `find_lockfile()` — Synchronous `Path::exists()` checks

**Location:** `fingerprint.rs:145-152`

```rust
fn find_lockfile(cwd: &str) -> Option<PathBuf> {
    for name in LOCKFILES {
        let p = std::path::Path::new(cwd).join(name);
        if p.exists() { return Some(p); }
    }
}
```

Up to 7 `exists()` syscalls in sequence. Called on every fingerprint computation.

**Impact:** Low. Typically 7 quick stat calls. But under high concurrency, these add up.

### 1.6 `plugin_runtime::discover()` — Blocking startup I/O

**Location:** `plugin_runtime.rs:35-48`

Uses `std::fs::read_dir` + `Command::output` synchronously. Only runs at startup so impact is negligible.

---

## 2. Database Performance

### 2.1 ✅ Significant Improvement: Connection Pooling

The database now uses `sqlx::SqlitePool` with WAL mode, foreign keys, `busy_timeout=5s`, and `max_connections=8`. This is a solid setup. Prepared statements are cached per connection by `sqlx` automatically.

### 2.2 Missing Index on `events.severity`

**Location:** `db.rs` migration v1

The `events` table has indexes on `run_id`, `repo_id`, `agent_id`, and `created_at`, but **no index on `severity`**. The `list_important_events` query (and the importance pipeline's event lookups) filter by `importance` and sort by `importance DESC, created_at DESC`. The `important_events` table does have `importance` in the sort but no dedicated index.

**Impact:** Low currently. The `important_events` table is likely small.

### 2.3 Dynamic SQL in `list_events` — No Prepared Statement Caching

**Location:** `db.rs:228-270`

```rust
let mut sql = String::from("SELECT ... FROM events WHERE ");
// Conditionally appends clauses based on which filters are provided
```

Each combination of (run_id, repo_id, agent_id) filters generates a different SQL string. With 8 possible combinations, `sqlx` may cache up to 8 prepared statements per connection. This is manageable but the dynamic approach prevents using a single cached statement.

**Impact:** Negligible for 8 combinations × 8 connections = 64 max cached statements. Not a real bottleneck.

### 2.4 No Batch Operations

The DB methods are all single-row or single-query operations. There's no batch insert for events, no bulk status update. Under high event throughput (many agents generating events), every event is an individual INSERT.

**Impact:** Low. Event insertion is not on any critical path that the user perceives.

### 2.5 `run_cache` Output Storage — Filesystem indirection

**Location:** `run_manager.rs:425-438`

DB cache entries store `output_path` (a filesystem path) rather than the actual output. Reading cached output requires:
1. DB query to get `output_path`
2. `std::fs::read()` to load the file
3. Gzip decompression if the file starts with `0x1f 0x8b`

This breaks the "single-source-of-truth" model and introduces filesystem dependency in what should be a DB operation. If output files are deleted or corrupted, cache entries silently return empty output.

**Impact:** Medium. The filesystem round-trip adds latency and failure modes. The current code handles this gracefully (returns `String::new()` on failure) but silently losing cached output is a functional bug.

### 2.6 No Transaction Scope in Critical Operations

The `run_or_join` flow does:
1. In-memory cache lookup
2. DB cache lookup
3. Possibly start a new run
4. Completion hook writes to both in-memory cache and DB

Steps 3-4 are not transactional. If the daemon crashes between spawning the process and persisting the cache entry, the DB will be missing a record. The in-memory and DB caches can diverge.

**Impact:** Low. Crash recovery is handled by the orphan reconciliation at startup, but the cache entries will be lost (not a data integrity issue, just a cache miss).

---

## 3. Memory Usage

### 3.1 ✅ Significant Improvement: LRU Cache with Capacity

The in-memory cache now uses `LruCache<CommandFingerprint, CachedResult>` with capacity 10,000. This bounds memory growth. Each `CachedResult` holds the full output string (up to 1MB) plus `changed_files` (unbounded `Vec<String>`). At 10,000 entries with 1MB each, theoretical worst case is **10 GB**. Realistic case: most cached outputs are small (1-100 KB).

**Remaining Risk:** No per-entry size accounting. If a build produces 500KB of output and there are 10,000 unique fingerprints, that's ~5GB. For a local macOS tool, this will trigger memory pressure.

**Fix:** Add a byte-based capacity limit (e.g., 100MB total) in addition to the entry count limit.

### 3.2 `CachedResult` Stores Full Output — Deduplication Failure

**Location:** `run_manager.rs:620-632`

When a run completes, the entire output (up to 1MB) is cloned into the `CachedResult`. This means:
- In-memory LRU cache holds up to 1MB per entry
- DB stores a path that points to a file (good)
- But the in-memory copy is a full duplicate of the on-disk output

**Impact:** Medium. The in-memory cache holds uncompressed output, while the DB stores compressed. This doubles memory for cached items.

### 3.3 Output Buffer — `String` Growth Pattern

**Location:** `supervisor.rs:314-318`

```rust
fn append_output(&self, line: &str) {
    let mut out = self.output.lock();
    if out.len() + line.len() < MAX_OUTPUT_BUFFER_BYTES {
        out.push_str(line);
        out.push('\n');
    }
```

Each `push_str` may reallocate. With high-frequency output (e.g., a build at `-j8`), this generates many small allocations that cause the `String` to repeatedly reallocate and copy.

**Impact:** Low. `String` growth is amortized O(1). But locking the `ParkingMutex` for each line append creates contention between the two reader tasks (stdout + stderr).

### 3.4 Broadcast Channel Capacity: 256

**Location:** `event_bus.rs:11`

`BROADCAST_CAPACITY: usize = 256`

The coalescing window (250ms) helps, but under a burst of rapid events from multiple agents, 256 slots fill quickly. For example:
- 5 agents × 10 file changes each = 50 `FileChanged` events in <1s
- Plus `RunStarted`, `RunCompleted`, `ResourcePressure` events

**Impact:** Low currently. Coalescing prevents most overflows. But the `Lagged` scenario means SSE clients may miss events — which is a functional concern, not just performance.

### 3.5 Mobile Gateway In-Memory State — Unbounded Growth

**Location:** `mobile_gateway.rs:70-87`

- `audit_log: RwLock<Vec<serde_json::Value>>` — entries are never evicted
- `approvals: RwLock<Vec<ApprovalEntry>>` — entries are never evicted
- `devices: RwLock<Vec<MobileDevice>>` — grows with each pairing
- `nonce_tracker: NonceTracker` with 10,000 capacity and rotation (acceptable)

The audit log and approvals lists grow forever. For long-lived daemons, this is a slow leak.

**Impact:** Low. Approval entries are typically small and infrequent. Audit log could grow if there's many device interactions. Fix: cap and rotate like the nonce tracker.

### 3.6 `completed` DashMap in ProcessSupervisor — No Eviction

**Location:** `supervisor.rs:53-54`

```rust
completed: Arc<DashMap<String, CompletedChild>>,
```

When a run finishes, its record is moved to `completed` and never removed. Each `CompletedChild` holds the full `RunSpec` and `output: String` (up to 1MB). Over hours of operation, this grows unbounded.

**Impact:** Medium-high for long-running daemons. 100 completed runs × 1MB = 100MB. This is a real leak.

**Fix:** Add a TTL or size limit. Evict entries after N minutes or when the map exceeds N entries.

---

## 4. Caching Correctness and Efficiency

### 4.1 ✅ Significant Improvement: LRU Cache

The migration from unbounded `DashMap` to `LruCache` with 10,000 capacity is a major improvement. However:

### 4.2 Cache Freshness Check Is Both Blocking and Incomplete

**Location:** `run_manager.rs:126-139`

The `is_fresh()` method:
1. Checks TTL (good, fast)
2. Stats every file in `changed_files` (blocking, slow)
3. **Does NOT check if git HEAD has changed** — a commit advancing HEAD would not invalidate the cache

Consider: Agent A runs `cargo test` → result cached. Agent B commits a new file. Agent C runs `cargo test` → gets stale result because:
- The cache TTL (300s) hasn't expired
- The `changed_files` list from the *original* run didn't include the new file
- `git rev-parse HEAD` now returns a different SHA, but the cache doesn't check it

This is a **correctness bug**: the cache can serve stale results when the repo advances via commits or rebases.

**Impact:** High. This is a functional correctness issue, not just performance. Users will see wrong/stale test results after commits.

**Fix:** Include `HEAD SHA` in the cache key (already computed during fingerprinting but not used in the cache lookup path). Or invalidate cache on `FileChanged` events from the watcher.

### 4.3 Cache Invalidation on File Change — Only Manual

The `invalidate_cache()` and `invalidate_repo_cache()` methods exist but are **never called** in the hot path. The watcher emits `FileChanged` events, but nothing connects those events to cache invalidation.

**Impact:** High (same root cause as 4.2). Cache entries persist until TTL expires regardless of file changes.

### 4.4 Cross-Worktree Cache Key Collision

**Location:** `run_manager.rs:383-388`

```rust
if entry.is_none() {
    let cross_key = fingerprint.cross_worktree_key();
    if cross_key != cache_key {
        entry = db.get_cache_entry(&cross_key).await.ok().flatten();
    }
}
```

The cross-worktree lookup uses `fingerprint_cross_worktree` which excludes CWD. This is correct for deduplication across worktrees of the SAME repo but **could match across completely different repos** if their git state happens to produce the same hash. The `hash_git_state` function includes HEAD + diffs + lockfile, which makes this extremely unlikely in practice but not cryptographically impossible.

**Impact:** Negligible. The hash includes enough entropy. Not a practical concern.

### 4.5 Fingerprint Asynchronicity — Good but Has a Mutex Bottleneck

**Location:** `fingerprint.rs:33-36`, `fingerprint.rs:82-96`

The fingerprint computation now uses `tokio::process::Command` and `tokio::join!` for concurrent git queries — this is a huge improvement over the old synchronous approach.

**However:** The `TOOLCHAIN_CACHE` uses `std::sync::Mutex`:

```rust
static TOOLCHAIN_CACHE: once_cell::sync::Lazy<Mutex<Option<ToolchainCache>>> = ...
```

This is a `std::sync::Mutex`, not `parking_lot::Mutex`. In an async context, holding a `std::sync::Mutex` across an `.await` point would be UB. In this case, the mutex is only held briefly (read/write of the cache struct), never across an await. But if someone adds an await inside the lock, it's a bug.

**Impact:** Low currently. The lock duration is nanoseconds (reading a `Vec` clone). No correctness risk as implemented.

### 4.6 Git Lock Contention on Fingerprinting

**Location:** `fingerprint.rs:56-77`

Every fingerprint call runs 4 concurrent git commands against the same repo. Git uses `.git/index.lock` for many operations. If multiple agents trigger fingerprinting simultaneously on the same repo, the concurrent `git diff --cached HEAD`, `git diff HEAD`, etc. can contend on the index lock.

**Impact:** Medium under high concurrency. Git commands will retry internally with `busy_timeout`, adding latency to every `run_or_join` call.

---

## 5. Concurrency Risks

### 5.1 ✅ Major Improvement: Async kill(), Notify-based Scheduler, OnceLock Auth

The three most critical concurrency hazards from the old review have been fixed:
- `supervisor.kill()` is now properly async
- Scheduler uses `Notify` instead of polling
- Auth token is cached in `OnceLock`

### 5.2 Lock Contention Between Output Readers

**Location:** `supervisor.rs:381-395`, `supervisor.rs:314-318`

Two spawned tasks (`read_output` for stdout, `read_output` for stderr) both call `child.append_output()` which acquires `ParkingMutex` on `self.output`. Each line of output competes for this lock. For high-frequency output producers (e.g., `cargo test -j8`), this creates contention.

**Impact:** Low. `ParkingMutex` is fast and the critical section is tiny (push to String). But it serializes output ordering — if stdout and stderr interleave rapidly, lock acquisition order doesn't match emission order.

### 5.3 `DashMap` Iteration Without Snapshot Semantics

**Location:** Multiple files

`DashMap::iter()` yields references that are individually locked but the iteration as a whole is not atomic. This means:
- `find_superset()` in `run_manager.rs:600-607` can miss a run that was inserted during iteration
- `check_orphans()` in `supervisor.rs` can see inconsistent state

**Impact:** Low. These are best-effort operations. Missing a superset or orphan during one check is fine — it will be caught on the next iteration.

### 5.4 `active_by_fingerprint` + `active_by_id` — Dual Map Consistency

**Location:** `run_manager.rs:88-89`

Two separate `DashMap`s track active runs. They are maintained manually with insert/remove calls. If any code path inserts into one but not the other (or removes from one but not the other), they diverge.

**Current State:** I verified all paths — every `insert` and `remove` on one map has a corresponding operation on the other. This is correct but fragile.

**Impact:** Low correctness risk, high maintenance risk. Any future change that forgets to update both maps creates an inconsistency.

**Fix:** Use a single map with a composite key, or wrap both in a struct that enforces consistency.

### 5.5 Scheduler `can_run_immediately()` — Non-Atomic Check-Then-Reserve

**Location:** `scheduler.rs:213-236`

The `acquire()` method:
1. Calls `can_run_immediately()` to check semaphore availability
2. If yes, calls `reserve_permits()` which actually acquires the semaphores

Between step 1 and step 2, another task could acquire the last permit, making `reserve_permits()` block on `.await`. This means `acquire()` might return a `Notify` that's already "ready" (from the `notify_one()` call) but the actual reservation hasn't completed yet.

**Impact:** Low. The semaphore acquisition will just wait until a permit is available. Not a deadlock, just unexpected blocking in what appears to be an "immediate" path.

### 5.6 Scheduler Queue Processor — Single-threaded Draining

**Location:** `scheduler.rs:264-283`

`process_queue()` processes entries one at a time in a loop. Under high queue depth, this means each entry must complete `reserve_permits()` before the next is considered. If the first entry is waiting for permits, no other queue entries are considered — even if later entries could run immediately on a different repo semaphore.

**Impact:** Low-medium. Queue depth is capped at 64, and entries typically acquire permits quickly. But a single heavy run blocking on the global semaphore blocks all subsequent queue processing.

---

## 6. Network I/O Patterns

### 6.1 ✅ Major Improvement: Bounded MCP Channels

All MCP transport channels are now bounded:
- `StdioTransport`: `channel(1024)` for incoming/outgoing
- `HttpTransport`: `broadcast(256)` + `channel(1024)`
- `InProcessTransport`: `channel(1024)` for both directions

The `send()` method now properly returns an error on backpressure instead of silently growing memory.

### 6.2 SSE Streaming — Full Buffer Clone on Every Request

**Location:** `supervisor.rs:363-376`

```rust
pub async fn stream_output(&self, run_id: &str) -> Option<tokio::sync::mpsc::Receiver<String>> {
    let child = self.children.get(run_id)?;
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let child_clone = child.clone();
    tokio::spawn(async move {
        let current = child_clone.output.lock().clone();  // <-- CLONES UP TO 1MB
        for line in current.lines() {
            if tx.send(line.to_string()).await.is_err() { return; }
        }
        child_clone.done().await;
    });
    Some(rx)
}
```

Each SSE subscription clones the entire output buffer (up to 1MB). For a long-running test with many subscribers (e.g., dashboard + mobile + CLI), this multiplies memory usage.

**Impact:** Medium. 5 concurrent stream requests × 1MB = 5MB per run. Acceptable but wasteful.

**Fix:** Use `Arc<str>` or `bytes::Bytes` for the output buffer to make cloning O(1).

### 6.3 SSE `/events` — No Backpressure from Slow Consumers

**Location:** `api.rs:490-510`

The SSE `/events` endpoint subscribes to the event bus broadcast channel. If a client reads slowly (e.g., a mobile device on poor WiFi), the broadcast channel drops events with `RecvError::Lagged`. The code logs a warning but the client never learns which events it missed — it just gets a `{"lagged": N}` message.

**Impact:** Low. SSE is inherently best-effort. But for audit/logging purposes, silently dropping events is undesirable.

### 6.4 MCP Sequential Processing Per Connection

**Location:** `server/protocol.rs:105-113`

```rust
while let Some(message) = self.transport.recv().await... {
    self.handle_message(message).await?;
}
```

Each MCP connection processes messages sequentially. No pipelining. If a `tools/call` takes 200ms (waiting for the daemon), the client can't send another request until the response arrives.

**Impact:** Low. MCP is designed for request-response, not high-throughput streaming. Single IDE clients won't notice.

### 6.5 Mobile Gateway — TCP Port Sharing No Backpressure

**Location:** `mobile_gateway.rs`

The mobile gateway uses a single TCP listener on port 9777 with standard axum handling. There's no connection limit. Under a misconfigured or malicious mobile client, many connections could exhaust file descriptors.

**Impact:** Negligible for the intended use case (0-5 mobile devices). But no hardening against abuse.

---

## 7. Scalability Ceilings

### 7.1 With 50+ Concurrent Agents

**Global Semaphore = 6:** At most 6 concurrent heavy runs. All others queue.

**Queue Depth = 64:** Only 64 queued runs before rejection. Agents 65+ get `RunOutcome::Rejected`.

**Fingerprint Git Contention:** 50 agents × 6 git commands = 300 concurrent subprocesses. Git's internal locking will serialize access. Expect 500ms-2s per fingerprint under this load.

**DB Throughput:** With 8 SQLite connections and WAL mode, ~500-1000 writes/sec. 50 agents each generating events, runs, and cache entries should be within capacity.

**Verdict:** The system will reject agent requests beyond queue capacity. Those agents will need retry logic. Response times will degrade as fingerprint contention and semaphore competition increase. **Hard ceiling at ~70 agents (6 running + 64 queued).**

### 7.2 With Large Repos (100k+ files)

**Fingerprint:** `git diff --name-only HEAD` on 100k files can take 5-10 seconds. The `is_fresh()` method would stat every `changed_file`, potentially 100k stats — this is catastrophic.

**Watcher:** FSEvents handles large repos well (kernel-level). The `should_include()` check does O(7) string comparisons per event, which is fine. But `canonicalize()` on each path is expensive.

**Verdict:** **The `is_fresh()` method will be the single worst performance issue with large repos.** It should never be allowed to stat 100k files. Cap `changed_files` and fall back to TTL-only freshness.

### 7.3 With Long-Running Builds (30+ min)

**Stall Detection:** `NO_OUTPUT_TIMEOUT_SECS = 300` (5 minutes). A 30-minute build with no output for 6 minutes will be killed. This is a **correctness issue** for slow compilation steps.

**Output Buffer:** Capped at 1MB. A verbose build producing >1MB of output will silently truncate. This is documented behavior but may surprise users.

**In-Memory State:** The `completed` DashMap never evicts. After 30 minutes, the completed run's output (1MB) stays in memory forever.

**Verdict:** Long-running builds require adjusting `NO_OUTPUT_TIMEOUT_SECS` (not configurable at runtime) and output buffer limits. The `completed` map leak will cause slow memory growth.

---

## 8. Algorithmic Complexity

### 8.1 Classifier — ✅ Fast

**Location:** `classifier.rs`

Pure string matching against known tool patterns. O(argv) per classification. The ecosystem-specific classifiers (JS, Python, Rust, Go, Swift, Java, Bazel, generic) cascade with `.or_else()`. Worst case: all 8 classifiers run, each O(argv). For typical commands (3-10 args), this is nanoseconds.

### 8.2 Fingerprint Computation — Medium

**Cost breakdown per `fingerprint()` call:**
1. `hash_identity()` — O(argv), nanoseconds
2. `hash_cwd()` — O(len(cwd)), nanoseconds
3. 4 git commands via `tokio::join!` — I/O bound, 10-500ms depending on repo size
4. `find_lockfile()` — up to 7 `exists()` syscalls, 0.1-1ms
5. `std::fs::read()` for lockfile — O(lockfile size), 0.1-5ms for typical locks
6. Toolchain cache check (TTL 60s) — cache hit is O(1), miss spawns 3 processes
7. BLAKE3 hash of diffs — O(diff size), typically 0.01-10ms

**Total:** Dominated by git I/O. Typical case: 20-100ms. Large repo with dirty tree: 200-1000ms.

**Key insight:** Every `run_or_join` call computes a fingerprint, even for "unknown" commands that bypass caching. This is wasteful — unknown commands could skip fingerprinting entirely.

### 8.3 `find_superset()` — Linear Scan

**Location:** `run_manager.rs:600-607`

```rust
fn find_superset(&self, fingerprint: &CommandFingerprint) -> Option<Arc<ParkingMutex<ActiveRun>>> {
    for entry in self.active_by_fingerprint.iter() {
        let fp = entry.key();
        if fp.is_superset_of(fingerprint) && fp != fingerprint { ... }
    }
}
```

Scans all active entries. With 6 concurrent runs, this is trivial. But the comparison `is_superset_of` calls `is_subset_of` which does string equality + enum equality — O(1) per comparison.

**Impact:** Negligible. Active entries are bounded by scheduler limits (max 6).

### 8.4 Event Coalescence — O(1) per variant

**Location:** `event_bus.rs:85-115`

Coalescence state is stored per variant name (10 variants) in a `DashMap`. Each `emit()` does a DashMap lookup + time comparison. `O(1)` amortized.

The `events_are_coalescable()` function does structural comparison on enum variants — O(field lengths). For `FileChanged` events, this is O(path length). Fast.

### 8.5 `should_include()` — O(7) per event

**Location:** `watcher.rs:266-274`

7 exclude pattern checks per file event. Each does a substring or suffix match. On a large monorepo with frequent file changes, this is called thousands of times per second.

**Impact:** Negligible. 7 string matches per event is trivial. But pre-compiling these into a `globset` would be even faster.

### 8.6 `invalidate_repo_cache()` — O(n × m) where n=cache entries, m=active runs

**Location:** `run_manager.rs:571-584`

Iterates all cache entries, and for each, does a DashMap lookup on `active_by_id`. This is O(n × O(1)) = O(n). With 10,000 cache entries, that's 10,000 DashMap lookups. Each is fast, but the `ParkingMutex` on the LRU cache is held for the entire iteration.

**Impact:** Medium. This method blocks the LRU cache for the duration of iteration. Should not be called frequently.

---

## 9. New Issues Not in Existing Review

### 9.1 `completed` DashMap Memory Leak (HIGH)

**Already described in §3.6.** The `ProcessSupervisor::completed` map grows without bound. Every finished run's output (up to 1MB) stays in memory forever.

### 9.2 Cache Serving Stale Results After Commits (HIGH)

**Already described in §4.2.** The cache freshness check doesn't account for git HEAD advancing. Post-commit test reruns get stale cached results.

### 9.3 `NO_OUTPUT_TIMEOUT_SECS` Not Configurable (MEDIUM)

**Location:** `supervisor.rs:22`

```rust
const NO_OUTPUT_TIMEOUT_SECS: u64 = 300;
```

Hard-coded. Long-running builds with quiet phases (e.g., Rust compilation of a large crate) will be killed after 5 minutes of no output.

**Fix:** Make this configurable via `DaemonState` settings or environment variable.

### 9.4 Stall Detection Uses `Instant::now()` as Approximation (LOW)

**Location:** `supervisor.rs:318`

Each `append_output` sets `*self.last_output_at.lock() = Instant::now()`. The stall detector checks if `last.elapsed() > NO_OUTPUT_TIMEOUT_SECS`. This is correct but acquires a `ParkingMutex` on every output line just to update the timestamp.

**Fix:** Use `AtomicInstant` from `parking_lot` or a `std::sync::atomic::AtomicU64` storing epoch millis.

### 9.5 `Output` Buffer Doesn't Separate stdout/stderr (LOW)

**Location:** `supervisor.rs:303`

Both stdout and stderr are merged into a single `output: String`. There's no way to distinguish error messages from regular output. This is a design limitation, not a bug.

### 9.6 `ProcessSupervisor::kill_repo()` Calls Sync `PathBuf::from` (NEGLIGIBLE)

Not a real issue — just noting for completeness.

---

## 10. Recommendations (Ranked by Impact)

| Priority | Action | Effort | Impact | Category |
|----------|--------|--------|--------|----------|
| **P0** | **Fix cache invalidation on git HEAD change** — include HEAD SHA in cache key or invalidate on `FileChanged` events | Medium | Squashes a correctness bug: stale cache hits after commits | Correctness |
| **P0** | **Evict `completed` DashMap entries** — add TTL or size cap (e.g., 100 entries, or 10 minutes) | Low | Prevents unbounded memory growth for long-lived daemons | Memory |
| **P1** | **Replace `std::fs::metadata` in `is_fresh()` with TTL-only or async** — cap `changed_files` at 50 entries; beyond that, fall back to TTL | Low | Eliminates blocking I/O on the hot cache-hit path; prevents catastrophic stat storms on large repos | Performance |
| **P1** | **Replace `std::process::Command` with `tokio::process::Command` in completion hook** — the `git diff --name-only HEAD` call should be async | Low | Prevents blocking a tokio worker thread on every run completion | Async |
| **P1** | **Add per-entry byte accounting to LRU cache** — cap total cached bytes (e.g., 100MB) not just entry count | Medium | Prevents OOM from many 500KB-1MB cache entries | Memory |
| **P2** | **Make `NO_OUTPUT_TIMEOUT_SECS` configurable** — read from settings or env var | Low | Prevents killing legitimate long-running builds with quiet phases | Config |
| **P2** | **Use `Arc<str>` or `Bytes` for output buffer** — make `stream_output()` clone O(1) | Medium | Eliminates 1MB clone per stream subscriber | Memory |
| **P2** | **Connect watcher `FileChanged` events to cache invalidation** — call `invalidate_cache()` when files referenced by cached results change | Medium | Fixes cache staleness in real-time without relying on TTL | Correctness |
| **P3** | **Evict mobile gateway `audit_log` and `approvals`** — cap and rotate like the nonce tracker | Low | Prevents slow memory leak in mobile gateway | Memory |
| **P3** | **Add index on `important_events.importance`** — speeds up `list_important_events` | Low | Minor DB optimization | Database |
| **P3** | **Skip fingerprinting for `CommandClass::Unknown`** — these commands bypass caching anyway | Low | Saves git syscall overhead on every unknown command | Performance |
| **P3** | **Pre-compile watcher exclude patterns** with `globset` crate | Low | Minor speedup on high-frequency file events | Performance |

---

## 11. Score

| Dimension | Score | Rationale |
|-----------|-------|-----------|
| **Async Runtime Correctness** | 8/10 | Major issues from old review are fixed. Remaining: blocking `fs::metadata` in hot path, blocking `std::process::Command` in completion hook |
| **Database Performance** | 8/10 | sqlx pool + WAL + prepared statements = solid. Minor: no batch ops, filesystem indirection for cached output |
| **Memory Management** | 6/10 | LRU cache is good, but `completed` DashMap leak, large per-entry sizes, and no byte-level accounting are real concerns |
| **Caching Correctness** | 5/10 | **Biggest weakness.** Cache can serve stale results after commits because freshness check doesn't account for HEAD advancement and watcher events don't trigger invalidation |
| **Concurrency Safety** | 8/10 | block_on hazard fixed. Scheduler Notify-based. Remaining: lock-ordering assumptions, non-atomic check-then-reserve |
| **Network I/O** | 8/10 | Bounded channels everywhere. SSE with lag tolerance. MCP is clean. Minor: 1MB buffer clone per stream subscriber |
| **Scalability** | 6/10 | Hard ceiling at queue_max=64. Single repo git contention under 50+ agents. Large-repo `is_fresh()` becomes catastrophic. Unbounded `completed` map |
| **Algorithmic Complexity** | 9/10 | Classifier is O(argv). Fingerprint is I/O-bound. All hot paths are reasonable. |

### Overall Performance/Scalability Score: **7/10**

The codebase has improved significantly since the original review — the critical issues (single DB mutex, block_on deadlock, unbounded channels, no cache eviction, scheduler busy-wait) are all fixed. The remaining issues are real but less severe:

- **The correctness bug** (stale cache after commits) is the most important finding. It's invisible to anti-pattern sweeps because it's a logic error, not a code smell.
- **The `completed` DashMap leak** is the most impactful performance issue for production deployments.
- **The blocking I/O in the hot path** (`is_fresh()`, `std::process::Command`) will cause degraded response times under concurrent load.

---

## 12. Summary of What the Existing Review Missed

1. **Cache correctness bug**: `is_fresh()` doesn't check git HEAD, causing stale results after commits. Not findable by anti-pattern sweep.
2. **`completed` DashMap leak**: Supervisor's completed runs never evicted. Old review focused on the LRU cache (now fixed) but missed this separate data structure.
3. **Blocking `std::process::Command` in completion hook**: Using sync git commands in an async task. Old review only identified `std::fs::metadata` but not this.
4. **Large-repo `is_fresh()` meltdown**: With 100k `changed_files`, the stat-per-file approach becomes catastrophic. Old review didn't quantify this.
5. **`NO_OUTPUT_TIMEOUT_SECS` hard-coded**: Kills legitimate slow builds.
6. **Cross-worktree cache key correctness**: The cross-worktree lookup path is subtle and could produce surprising results.
7. **All 9 corrections to outdated findings** (§0 above).
