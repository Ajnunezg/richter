# Richter Architecture & Systems Design Review

**Reviewer**: Senior Systems Architect
**Date**: 2026-05-05
**Scope**: Full source code review of all 4 crates, 8 ADRs, Cargo.tomls, integrations/

---

## Overall Architecture Score: 4/10

This is **prototype-grade** code written with excellent *intentions* and solid ADR thinking, but the implementation is a first-draft that hasn't been through a single refactoring pass. The ADRs describe a world-class system; the code delivers a hackathon demo. Every crate has the same architectural sin: too much undifferentiated code stuffed into too few modules, zero interface abstraction between crates, and a pervasive "string over type" anti-pattern that will cause cascading failures the moment this hits real multi-agent workloads.

The code compiles. It probably runs. It is not ready for production use by any reasonable definition. The rewrite risk is **high** — not because the architecture is wrong, but because the code has accumulated technical debt in the wrong places and a 5x to 10x code growth (to support the ADR vision) will be structurally blocked by current choices.

---

## Key Strengths

### 1. ADRs are genuinely excellent
The eight ADRs (`adr/0001` through `0008`) are the strongest artifact in this codebase. They demonstrate clear systems thinking:

- **ADR 0001** (system architecture): correct decision to split SwiftUI + Rust daemon. The Unix socket choice is right. The monorepo justification is sound.
- **ADR 0002** (fingerprinting): the conservative "false misses are acceptable, false hits are not" philosophy is correct. BLAKE3 choice is defensible.
- **ADR 0003** (no Endpoint Security): brutally honest about deployment friction. Right call for v1.
- **ADR 0004** (LLM pipeline): three-tier design with budget limits is production-grade thinking.
- **ADR 0005-0008** (mobile): pairing ceremony, trust model, React Native choice — all well-reasoned.

These ADRs demonstrate the architect *understands* the problem domain. The gap between ADR quality and code quality is the central finding of this review.

### 2. Crate dependency direction is clean
```
richter-core  ←  richter-daemon
     ↑                ↑
richter-cli     richter-mcp
```

No cycles. `richter-core` has zero internal dependencies — this is correct. The `richter-core` dependency sits at the DAG root. Each consumer pulls only what it needs. The workspace dependency management in root `Cargo.toml` is clean.

### 3. Classifier is comprehensive and well-tested
`crates/richter-core/src/classifier.rs` covers 7+ ecosystems (JS/TS, Python, Rust, Go, Swift/Xcode, Java, Bazel) with 40+ unit tests. The `ClassifiedCommand` return type is well-structured. The `normalize_argv` pass-through logic handles the `richter run --` wrapper correctly.

This is the **best single module in the codebase** by a wide margin. It shows what the rest of the code *could* look like with discipline.

### 4. SQLite schema is thoughtful
`crates/richter-core/src/db.rs` migration v1 creates 15+ tables with proper foreign keys, indexes on hot paths (runs by repo_id, fingerprint, status), and a clean migration engine. WAL mode is enabled. This is correctly scoped.

### 5. Transport abstraction in MCP crate
`crates/richter-mcp/src/transport.rs` defines a clean `Transport` trait with three implementations (Stdio, HTTP/SSE, InProcess). This is the right level of abstraction. The `InProcessPeer` for daemon embedding is the correct architecture — avoiding serialization overhead when the MCP server is co-located.

---

## Key Weaknesses

### 1. String-over-type anti-pattern is pervasive and dangerous

This is the single biggest architecture problem. Despite `richter-core/src/models.rs` defining strong typed wrappers (`RepoId`, `RunId`, `AgentId`, etc.) and proper enums (`CommandClass`, `RunStatus`, `EventKind`, `DecisionOutcome`), **no downstream crate uses them**.

Evidence — every function signature in the daemon:

```rust
// richter-daemon/src/scheduler.rs:104
pub async fn acquire(
    &self,
    run_id: &str,     // Should be RunId
    repo: &str,       // Should be RepoId or &Path
    command: &str,
    class: ResourceClass,
) -> Option<Arc<Notify>>

// richter-daemon/src/run_manager.rs:202
pub async fn run_or_join(&self, spec: RunSpec) -> Result<RunOutcome>

// RunSpec itself (supervisor.rs:42):
pub struct RunSpec {
    pub run_id: String,         // Should be RunId
    pub repo: String,           // Should be PathBuf
    pub command: String,
    pub classification: String, // Should be CommandClass
    pub resource_class: String, // Should be ResourceClass
    // ...
}
```

`RunSpec.classification` is a `String` like `"test"`. If someone fat-fingers `"tset"`, the system silently treats it as unknown — no compile-time error. `CommandClass` exists in `richter-core` as a perfectly good enum. Nothing uses it outside of `models.rs` and `classifier.rs`.

The `models.rs` file defines beautiful typed wrappers. They collect dust. Every inter-module boundary passes `String` where a `RunId`, `RepoId`, or enum variant should be.

**This is not a style preference. It is a correctness risk.** When `run_manager.rs` calls `scheduler.acquire(run_id, repo, ...)`, and `run_id` is just a `&str`, nothing prevents callers from passing a repo name as the run_id. The compiler can't help.

### 2. richter-core models are unused by their own consumers

`richter-core/src/models.rs` defines 18 structs, 8 enums, and 15 type aliases. The daemon crate imports `richter-core` and uses exactly **zero** of these types in its public API. Instead, every daemon module redefines equivalent types:

| richter-core type | Daemon equivalent | Where |
|---|---|---|
| `models::Run` | `RunRow` (db.rs) + `RunSpec` (supervisor) + `ActiveRun` (run_manager) | Three files |
| `models::Event` | `DaemonEvent` (event_bus) | Separate enum |
| `models::EventKind` | `DaemonEvent` variants string-matched | event_bus.rs |
| `models::CommandClass` | literal string `"test"`, `"build"` | Everywhere |
| `models::RunStatus` | literal string `"running"`, `"passed"` | Everywhere |
| `models::DecisionOutcome` | `RunOutcome` enum (run_manager) | Different names, same semantics |
| `models::ResourceClass` | `ResourceClass` (scheduler) | **Redeclared in daemon** |
| `models::ResourcePressure` | `ResourceSnapshot` (scheduler) | Different name, same concept |

The `ResourceClass` enum is defined in BOTH `richter-core/src/resource.rs` AND `richter-daemon/src/scheduler.rs` with different variants and semantics. The core version has `Unknown`; the daemon version doesn't. This is not DRY — it's actively divergent.

**Impact**: When the team needs to add a new `CommandClass` variant (e.g., `SecurityScan`), they must update it in `models.rs`, then hunt down every string match in `run_manager.rs` (`spec.classification == "unknown"`), `classifier.rs`, and `scheduler.rs`. This is how production incidents happen.

### 3. Scheduler busy-wait is a CPU-wasting anti-pattern

The ADR correctly identifies this, but the code still ships it. From `richter-daemon/src/scheduler.rs` lines 130-147, the `acquire()` method spins a tokio task that **polls every 100ms**:

```rust
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;

        if monitor_owned.is_under_pressure() {
            continue;
        }

        let permit = concurrency.global.clone().try_acquire_owned().ok();
        if permit.is_none() {
            continue;
        }
        // ...
    }
});
```

With 64 queued items, that's 640 tokio wake-ups per second doing `try_acquire_owned()` (which fails), checking `is_under_pressure()` (also fails). This is pure scheduler overhead delivering zero value.

The fix (a `Notify`-based approach) is well-known. The `tokio::sync::Notify` type exists. It's not used here.

### 4. Run manager has the same polling pathology

`run_manager.rs` `start_new()` spawns a completion hook (line ~265):

```rust
tokio::spawn(async move {
    loop {
        if let Some(code) = supervisor.exit_code(&run_id_for_cache) {
            // ...
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
});
```

Every active run has a background task polling exit_code every 200ms. At 10 concurrent runs, that's 50 polls/sec. The correct pattern is `tokio::sync::watch` or `Notify` — the supervisor already has a `done_tx: watch::Sender<bool>` field. The run manager doesn't subscribe to it.

### 5. Supervisor.kill() blocks the async runtime — a concurrency landmine

From `supervisor.rs` lines ~265 (inside `SupervisedChild::kill()`):

```rust
pub fn kill(&self) -> Result<()> {
    // ...
    if let Some(mut child) = self.child.lock().take() {
        let _ = child.start_kill();
        let rt = tokio::runtime::Handle::current();
        let _ = rt.block_on(child.wait());  // ← BLOCKS THE ASYNC THREAD
    }
}
```

`kill()` is a **synchronous method** that calls `tokio::runtime::Handle::current().block_on()`. If called from within an async context (and it is — `run_manager.rs` calls `self.supervisor.kill_run(run_id).await`, which calls `child.kill()`), this blocks a tokio worker thread for the duration of the child process wait (potentially seconds).

Under heavy load with many simultaneous kills (shutdown drain), this can exhaust the tokio blocking thread pool and cause a deadlock. The ADR identifies this as P0. The code hasn't been fixed.

### 6. In-memory cache has no eviction — guaranteed memory leak

`run_manager.rs`:
```rust
cache: Arc<DashMap<CommandFingerprint, CachedResult>>,
```

No TTL (other than freshness check on lookup), no size cap, no eviction policy. The daemon is designed to run as a login item — potentially for days or weeks. Every distinct fingerprint (every unique command+context combination) gets cached forever. On a busy developer machine running dozens of distinct commands across branches, this `DashMap` will grow to tens of thousands of entries.

Cached results hold full `String` output buffers. A `cargo build` can produce megabytes of output. 1000 cached builds × 1MB = 1GB leaked in the in-memory cache alone.

The `db.rs` persistent cache has a proper eviction system. The in-memory one doesn't.

### 7. Mobile Gateway is wired but non-functional

`mobile_gateway.rs` is 450+ lines implementing a pairing protocol, device registry, scope checking, and API routes. It uses Ed25519 signing keys. It has proper tests.

But wiring it to the daemon is a stub. The `/mobile/v1/now` handler returns hardcoded zeros. The `/mobile/v1/runs` handler returns run IDs but with empty `repo`, `command`, `classification`, and `exit_code` fields. The event bus integration (`collect_top_event`) drains the broadcast channel on every call (`try_recv()` — a destructive read).

This creates a nasty correctness bug: calling `GET /mobile/v1/now` consumes an event from the broadcast channel via `try_recv()`, starving the actual /events SSE stream.

### 8. Event bus broadcast channel has capacity 256 — undersized

`event_bus.rs`:
```rust
const BROADCAST_CAPACITY: usize = 256;
```

Under burst load (many file watch events, multiple agent starts), slow consumers are dropped with `RecvError::Lagged`. The lagged consumer gets a single `{"lagged": N}` SSE event — all intermediate events are silently lost. For a coordination plane, losing events means agents miss conflict notifications, cache invalidation signals, and status transitions.

The ADR correctly identifies this. The fix is a one-line constant change. It hasn't been done.

### 9. Watcher has a correctness bug: wrong repo resolution

From `watcher.rs`:
```rust
fn find_repo_for_path(&self, path: &Path) -> Option<String> {
    for entry in self.active_roots.iter() {
        let root_path = PathBuf::from(entry.key());
        if path.starts_with(&root_path) {
            return self.repo_states.iter().next().map(|e| e.key().clone());
        }
    }
    None
}
```

When a path matches a watched root, this returns the **first repo in the DashMap**, not the repo matching that root. If Richter watches two repos (`/projects/repo-a` and `/projects/repo-b`), a file change in repo-a could be attributed to repo-b. The ADR calls this out. It's still in the code.

### 10. Auth middleware reads token from disk on every request

```rust
// api.rs: auth_middleware
async fn auth_middleware(...) {
    let expected = match load_auth_token(&state.token_path) {
        Ok(t) => t,
        // ...
    };
```

`load_auth_token()` calls `std::fs::read_to_string()` — a blocking synchronous IO operation — inside an **async** axum middleware that runs on **every request**. The token is static for the lifetime of the daemon process. It should be cached in `DaemonState` at startup.

### 11. CLI client sends HTTP/1.1 raw bytes over a Unix socket with manual construction

`client.rs` manually assembles HTTP requests with string concatenation:
```rust
let mut req = String::new();
req.push_str(&format!("{http_method} {path} HTTP/1.1\r\n"));
req.push_str("Host: localhost\r\n");
// ...
```

This works for now. It won't survive: chunked transfer encoding, keep-alive connection reuse, streaming responses > memory capacity, proper error propagation from HTTP status codes (the client ignores status codes entirely — `read_to_end` just returns whatever bytes come back). This is a `hyper` client with a Unix socket connector waiting to happen.

---

## Specific Architectural Risks

### Risk 1: Schema lock-in via string-typed APIs
**Severity**: Critical
**Evidence**: Every public method in the daemon takes `String`/`&str` for entity IDs. Changing the ID format (e.g., from UUID to ULID, or adding a shard prefix) requires touching every call site. No type system assistance.

### Risk 2: Tokio blocking thread pool exhaustion
**Severity**: High
**Evidence**: `supervisor.kill()` calls `block_on()`. `auth_middleware` calls blocking `read_to_string`. `fingerprint.rs` runs 6+ `std::process::Command` calls synchronously inside the fingerprint computation path (which is called from `run_manager.run_or_join()` — an async function, though it's not awaited). SQLite queries hold a `Mutex<Connection>` with synchronous rusqlite calls inside async handlers.

The default tokio blocking thread pool is 512 threads. Under normal loads this won't be hit. Under pathological load (50+ concurrent `run_or_join` requests each fingerprinting + DB writes + potential blocking kills), exhaustion is possible.

### Risk 3: No backpressure on MCP unbounded channels
**Severity**: Medium (low probability, high impact)
**Evidence**: `transport.rs` uses `mpsc::unbounded_channel` for all three transport modes. An aggressive MCP client (buggy agent) could flood the channel faster than the server processes messages. Memory grows without bound until OOM kill.

### Risk 4: Divergent type definitions between core and daemon
**Severity**: Medium
**Evidence**: `ResourceClass` defined twice with different semantics. `RunOutcome` in run_manager vs `DecisionOutcome` in core — different names, different variants, same domain concept. When the team needs to add `SecurityAudit` as a run outcome, which enum gets it? Both? Neither?

---

## Scaling Ceiling Assessment

### Can it scale to 10 concurrent agents? **Yes, barely.**
The scheduler hard-limits heavy runs to 3 global / 1 per repo. 10 agents doing mostly light work (lint, format) would work. The DB mutex would be noticeable but not blocking. The busy-wait overhead (640 polls/sec at queue max) is wasteful but within a single machine's capacity.

### Can it scale to 50 concurrent agents? **No.**
- 50 agents submitting commands would hit the DB mutex constantly — every cache lookup serializes.
- Scheduler queue at capacity (64) means 64 polling tasks each at 100ms = 640 tokio wake-ups/sec.
- Event bus capacity 256 — with 50 agents each producing multiple events, lagged consumers would lose coordination signals within seconds.
- In-memory cache unbounded growth would consume hundreds of MB within hours.
- Git fingerprinting (6 subprocess spawns per command) would contend for `.git/index` lock. 50 agents fingerprinting simultaneously = git process storms.

### Realistic limit: ~15-20 agents doing mixed light/heavy work before UX degrades to >1s `run_or_join` latency.

---

## Concurrency Concerns

### 1. `block_on` in `kill()` — the most dangerous pattern in the codebase
If two concurrent `kill_run()` calls trigger `kill()` on the same child, the second `block_on(child.wait())` will panic (the child handle was already taken by the first). The `killed` flag is checked after taking the child handle — there's a TOCTOU race.

### 2. DashMap + inner Mutex layering is correct but fragile
The run manager pattern (`DashMap<Fingerprint, Arc<Mutex<ActiveRun>>>`) works because the lock order is consistent: acquire outer DashMap entry first, then inner Mutex. If any future code path acquires the inner Mutex then attempts a DashMap write, it deadlocks. This is undocumented.

### 3. Event bus coalescence is best-effort, not guaranteed
The `DashMap<String, CoalescenceState>` in `event_bus.emit()` checks for duplicates within a 250ms window. Under concurrent emits from multiple tokio tasks, two identical events can race past the coalescence check. This isn't a correctness bug (coalescence is advisory) but it means the rate-limiting guarantees in the ADR aren't enforced.

---

## Does the Architecture Accelerate or Slow the Team?

**It slows the team.**

The ADRs set a high bar. The code doesn't meet it. Every new feature will require:

1. Deciding whether to use the (unused) richter-core types or extend the daemon-local equivalents.
2. Propagating string-based IDs through multiple layers with no compiler verification.
3. Working around the polling loops instead of fixing them.
4. Adding to the growing list of "ADR says X, code does Y" discrepancies.

A team of 2-3 experienced Rust developers could fix the top 5 issues in 2-3 weeks. After that, velocity would improve significantly because the type system would carry more of the cognitive load. Until then, every PR review is a game of "does this string match the expected format?"

---

## Hidden Rewrite Risk

**High — 60-70% of daemon code needs structural refactoring, not a rewrite.**

Specifically:

| Module | Risk | Notes |
|---|---|---|
| `run_manager.rs` | **High** | Fingerprinting, caching, subset/superset logic all intertwined. Can't fix one without touching all. |
| `scheduler.rs` | **High** | Polling loop is baked into the `acquire()` API contract. Changing to event-driven requires API surface change. |
| `supervisor.rs` | **Medium** | `kill()` needs async conversion — signature change ripples through all callers. |
| `api.rs` | **Low** | Axum handlers are well-factored. Token caching is a one-line fix. |
| `event_bus.rs` | **Low** | Capacity bump is trivial. Coalescence is self-contained. |
| `watcher.rs` | **Medium** | Buggy `find_repo_for_path` needs rewrite. Rest of watcher is clean. |
| `classifier.rs` | **Very Low** | Best module. Add parsers without touching existing ones. |
| `fingerprint.rs` | **Medium** | Blocking git commands in sync code. Needs async or thread pool. |
| `mobile_gateway.rs` | **Medium** | Stub-wired to daemon. Needs real integration but the pairing protocol is solid. |
| `db.rs` | **Low** | Schema is good. Migration engine is clean. Would benefit from sqlx async but rusqlite + Mutex works. |

The good news: none of this requires architectural re-envisioning. The architecture in the ADRs is correct. The code just needs to catch up. A disciplined refactoring pass focused on (a) typed interfaces, (b) event-driven wakeups, (c) cache eviction, and (d) token caching would bring this from prototype-grade to startup-grade.

---

## Direct Quotes & Patterns Supporting Judgment

### The String Disease

From `run_manager.rs` — the core orchestration logic:

```rust
// Uses literal string comparison for control flow
if spec.classification == "unknown" {
    // ...
}
if self.is_destructive(&spec.command) && !spec.force && !spec.preview {
    // ...
}
```

`spec.classification` could be `CommandClass::Unknown` — a compile-time exhaustive match. Instead it's a runtime string compare. If someone passes `"Unkown"` (typo), the system silently skips the pass-through logic.

### The Duplicate Type Problem

From `richter-core/src/resource.rs`:
```rust
pub enum ResourceClass {
    HeavyBuild, HeavyTest, LightLint, Install, DevServer, Unknown,
}
```

From `richter-daemon/src/scheduler.rs`:
```rust
pub enum ResourceClass {
    HeavyBuild, HeavyTest, LightLint, Install, DevServer,
}
```

No `Unknown` variant in the daemon. Different crate, different semantics, same name. Import collision waiting to happen.

### The Busy-Wait Signature

From `scheduler.rs`, the acquire method's spawned polling task:

```rust
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        // poll, check, poll, check, poll, check...
    }
});
```

This is the literal textbook anti-pattern for async Rust. `tokio::sync::Notify` exists for exactly this use case.

### The Stub That Lies

From `mobile_gateway.rs`, the `now_handler`:

```rust
async fn now_handler(...) -> Json<MobileNowResponse> {
    Json(MobileNowResponse {
        daemon_ok: state.event_bus.is_some(),
        active_runs: state.run_manager.as_ref().map_or(0, |rm| rm.active_runs().len()),
        queued_runs: 0,     // hardcoded
        cpu_percent: 0.0,   // hardcoded
        memory_percent: 0.0, // hardcoded
        duplicate_work_saved: 0, // hardcoded
        agent_conflicts: 0,     // hardcoded
        approvals_pending: 0,   // hardcoded
        // ...
    })
}
```

The mobile "Now" view — the single most important screen for the companion app — returns five hardcoded zeros. This is not a stub. It's a placeholder that will ship if someone enables `RICHTER_MOBILE_ENABLED=true` without checking.

### The Correct Pattern (Exists, Unused)

From `richter-core/src/models.rs`:

```rust
pub type RunId = Uuid;
pub type RepoId = Uuid;
```

This is the right approach. Newtype wrappers would be even better (`pub struct RunId(Uuid);`) but the type aliases at least signal intent. **They are used nowhere outside models.rs.**

---

## Recommendations (Prioritized)

### P0 — Fix before any production use
1. **Convert RunSpec fields to typed enums**: `classification: CommandClass`, `resource_class: ResourceClass`. Remove all string comparisons on classification.
2. **Make supervisor.kill() async**: remove `block_on()`. This is a crash/deadlock risk.
3. **Add LRU eviction to in-memory cache**: Cap at 1000 entries or 100MB total output. Or both.
4. **Cache auth token in DaemonState**: stop reading from disk on every request.

### P1 — Fix before scaling past 5 agents
5. **Replace scheduler polling loop with Notify-based wakeups**: eliminates 640 wakeups/sec overhead.
6. **Replace run manager polling loop with watch channel subscription**: the `done_tx` field already exists.
7. **Bump event bus capacity to 1024**: one-line change, documented in ADR, not done.

### P2 — Fix before adding features
8. **Unify ResourceClass definition**: delete the daemon copy. Import from richter-core.
9. **Fix watcher repo resolution bug**: `find_repo_for_path` must return the correct repo.
10. **Wire mobile gateway to real daemon metrics**: the hardcoded zeros make the feature misleading.

### P3 — Nice to have
11. **Make MCP channels bounded** (1024): prevent memory growth from aggressive clients.
12. **Use hyper client for daemon communication**: replace manual HTTP/1.1 string construction.
13. **Add prepared statement caching to SQLite**: rusqlite supports `CachedStatement`.
14. **Convert RunSpec.repo from String to PathBuf**: path semantics deserve path types.

---

## Final Verdict

The Richter architecture — as designed in the ADRs — is a **7/10**. Clean crate boundaries, correct IPC choices, good DB schema, thoughtful mobile trust model.

The Richter implementation — as found in the source code — is a **4/10**. Typed models go unused, strings carry domain semantics, polling loops waste CPU, in-memory caches leak, a blocking `block_on()` lurks in the kill path, the mobile gateway returns hardcoded zeros, and the watcher misattributes events to the wrong repo.

The team should spend 2-3 weeks on a disciplined refactoring pass — no new features — focused on the P0 and P1 items above. The result would be a solid **7/10** codebase that matches the **7/10** architecture in the ADRs. From there, feature velocity would accelerate significantly.

Shipping this as-is to real users with real multi-agent workloads would result in: memory leaks (cache), UI freezes (block_on in kill), incorrect conflict notifications (watcher bug), misleading mobile views (hardcoded zeros), and eventual OOM kills (unbounded MCP channels + unbounded cache).

**This is not ready. It is close to being ready. Two to three weeks of focused refactoring closes the gap.**
