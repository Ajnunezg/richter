# Richter Architecture & Systems Design Review

**Scope:** Full Rust workspace (`richter-core`, `richter-daemon`, `richter-cli`, `richter-mcp`) + ADRs
**Date:** 2026-05-17
**Assessed by:** Sub-agent Architecture Reviewer

---

## 1. Executive Summary

Richter is a **well-above-average startup-grade** architecture. The crate layering is clean, dependencies flow in one direction, and domain boundaries are mostly respected. Technology choices (Tokio, Axum, SQLite/WAL, BLAKE3, serde) are battle-tested and appropriate for the problem. The author clearly understands both Rust async patterns and macOS-native constraints.

That said, there are **three classes of concern** that will compound over 12–24 months if left unaddressed:

1. **Mega-module syndrome**: several files exceed 1,500 LOC and serve multiple responsibilities.
2. **Stubbed AI pipeline**: the importance engine’s model tiers are placeholder logic.
3. **God-object state**: `DaemonState` in `api.rs` is already a 12-field struct with mixed lifecycle concerns.

Verdict: **the architecture will accelerate the team for ~6 months, then slow it down as the mega-modules resist refactoring.** It is not prototype-grade, but it is not yet world-class.

---

## 2. Crate Boundary Assessment

### 2.1 Dependency Graph

```
richter-core
    ↑           ↑           ↑
richter-daemon   richter-cli   richter-mcp
```

**No cycles. Unidirectional. Correct.**

| Crate | Depends on | Notes |
|-------|------------|-------|
| `richter-core` | None (workspace deps only) | Pure library. No awareness of daemon, CLI, or MCP. |
| `richter-daemon` | `richter-core` (path dep) | Only consumer that needs DB + models + fingerprinting. |
| `richter-cli` | `richter-core` (path dep) | Uses models + client logic; talks to daemon over UDS, not as a lib dep. |
| `richter-mcp` | `richter-core` (path dep) | Uses models; implements MCP protocol on top. |

**Evidence:**
```
richter-daemon/src/main.rs:10    use richter_core::models::RunStatus;
richter-daemon/src/scheduler.rs:17 pub use richter_core::models::ResourceClass;
richter-daemon/src/run_manager.rs:23 use richter_core::db::Database;
richter-cli/src/main.rs:10        use richter_cli::client;
richter-mcp/src/lib.rs            pub mod daemon; // no daemon crate dep
```

**Risks:** None for cycles. However, `richter-core` is doing **too much**: it owns models, DB schema, SQLx queries, config parsing, command classification, fingerprinting, git detection, redaction, resource scheduling primitives, and retry logic. That is at least three subdomains crammed into one crate.

---

## 3. Domain Separation Quality

### 3.1 What is Good

- **Command classification** (`classifier.rs`, 867 LOC) is isolated and deterministic. It knows about ecosystems (JS, Python, Rust, Go, Swift, Java, Bazel) but does not leak that knowledge into the scheduler.
- **Fingerprinting** (`fingerprint.rs`, 398 LOC) is pure: BLAKE3 + git state. No daemon logic.
- **Secrets redaction** (`redact.rs`, 313 LOC) is self-contained and conservative (over-redacts).
- **Config** (`config.rs`, 732 LOC) is a single deserialization target. Clean `serde` structs.

### 3.2 What is Borderline

- **`models.rs` (937 LOC)** is a monolith. Every domain identifier (`RepoId`, `RunId`, `EventId`, `DecisionId`, `LeaseId`, `CacheEntryId`, `ImportantEventId`, `PluginManifestId`, `SettingId`) lives in one file. This works at small scale but becomes a merge-conflict factory as the team grows.
- **`db.rs` (1,549 LOC)** contains schema definitions, migrations, **and** 30+ CRUD methods across 15 tables. No repository pattern, no transaction wrapper, no query builder abstraction. Every method is a raw `sqlx::query` or `sqlx::query_as`. This is cohesion by coincidence, not by design.

### 3.3 What is Weak

- **`api.rs` (1,061 LOC)** in the daemon mixes:
  - HTTP routing (Axum)
  - Auth middleware (Bearer token + constant-time comparison)
  - `DaemonState` god object
  - Model-call budget (circuit-breaker-ish)
  - Install status struct
  - SSE streaming
  - Mobile-gateway forwarding

  That is at least three separate concerns: transport, auth, and application state.

---

## 4. Coupling / Cohesion Analysis

### 4.1 High Coupling — God Object

**File:** `crates/richter-daemon/src/api.rs`
**Struct:** `DaemonState`

```rust
pub struct DaemonState {
    pub event_bus: crate::event_bus::EventBus,
    pub run_manager: Arc<crate::run_manager::RunManager>,
    pub scheduler: Arc<crate::scheduler::Scheduler>,
    pub supervisor: Arc<crate::supervisor::ProcessSupervisor>,
    pub token_path: PathBuf,
    pub auth_token: Arc<std::sync::OnceLock<String>>,
    pub repos: ParkingMutex<Vec<RepoEntry>>,
    pub settings: ParkingMutex<HashMap<String, serde_json::Value>>,
    pub install_status: ParkingMutex<InstallStatus>,
    pub mobile_state: Option<Arc<MobileGatewayState>>,
    pub model_call_budget: Arc<parking_lot::Mutex<ModelCallBudget>>,
    pub db: Arc<richter_core::db::Database>,
    pub watcher_healthy: Arc<std::sync::atomic::AtomicBool>,
}
```

Every API handler depends on all of this. If you want to add a new setting, you touch `DaemonState`. If you want to add a new service, you add a field here. This is the exact pattern that turns a 1,000-line file into a 3,000-line file.

### 4.2 High Coupling — Raw SQL in Business Logic

**File:** `crates/richter-core/src/db.rs`

Every table has its own `insert_*`, `update_*`, `get_*`, and `list_*` method. There is no `RunRepository` or `EventRepository`. The `run_manager` and `scheduler` both talk to the same `Database` handle and embed SQL semantics indirectly through method names.

**Concrete evidence:**
```rust
// db.rs:142
pub async fn update_run_status(&self, id: &str, status: RunStatus, ...) -> anyhow::Result<()> {
    sqlx::query("UPDATE runs SET status = ?2, exit_code = ?3, ...").bind(id)...
}
```

### 4.3 Medium Coupling — CLI Knows Daemon Response Shape

**File:** `crates/richter-cli/src/main.rs`

The CLI parses daemon responses by matching on `resp["type"].as_str()` and reaching into JSON fields (`resp["output"]`, `resp["exit_code"]`). There is no shared response type in `richter-core`. If the daemon changes its JSON shape, the CLI breaks at runtime.

### 4.4 Cohesion Wins

- `event_bus.rs` (390 LOC) is tightly cohesive: one enum, one bus, one filter, coalescence logic, and tests. **This is the best module in the repo.**
- `scheduler.rs` (525 LOC) is focused: resource monitor, concurrency limits, queue. Could be smaller, but its concerns are related.
- `supervisor.rs` (759 LOC) is mostly about spawning + monitoring children. A bit large, but coherent.

---

## 5. Extensibility Evaluation

### 5.1 Plugin Runtime — Extensible in Theory, Stub in Practice

**File:** `crates/richter-daemon/src/plugin_runtime.rs` (~90 LOC)

The plugin system scans `~/.richter/plugins/` and executes `--richter-manifest`. It returns a `Plugin` struct with `name`, `version`, `capabilities`. There is:
- No WASM sandbox
- No gRPC/IPC interface
- No capability enforcement beyond a string list
- **No tests**

Verdict: A **vibe-coded stub**. It will not survive real plugin authors.

### 5.2 Importance Engine — Traits are Good, Implementation is Thin

**File:** `crates/richter-daemon/src/importance/pipeline.rs` / `parsers.rs`

- `OutputParser` trait exists. New parsers can be added via `add_parser()`.
- `cheap_model_boost()` and `frontier_model_boost()` are **noop stubs**:
  ```rust
  fn cheap_model_boost(&self, severity: Severity, result: &ParseResult) -> Severity {
      if result.failure_count > 10 && severity < Severity::Critical { Severity::Critical } else { severity }
  }
  fn frontier_model_boost(&self, severity: Severity, _result: &ParseResult) -> Severity { severity }
  ```
  This contradicts the sophistication described in **ADR-0004**.

### 5.3 MCP Bridge — Clean Trait Surface

**File:** `crates/richter-daemon/src/mcp_bridge.rs`

```rust
#[async_trait::async_trait]
pub trait ToolHandler: Send + Sync {
    fn definition(&self) -> McpTool;
    async fn invoke(&self, args: serde_json::Value) -> ToolCallResult;
}
```

This is the right abstraction. The built-in tools (`ListRunsTool`, `HealthTool`, `RunOrJoinTool`) are small and composable. The `richter-mcp` crate implements the actual MCP protocol lifecycle separately, which is architecturally sound.

### 5.4 Config — Extensible but Bloated

**File:** `crates/richter-core/src/config.rs`

`RichterConfig` has 14 top-level fields. Adding a new feature means adding another field and another `#[serde(default)]`. At some point this needs to be split into sub-configs per domain.

---

## 6. Hidden Fragilities & Scaling Risks

### 6.1 Persistence Layer

- **SQLite WAL mode** with `max_connections = 8`. This is fine for a single-machine daemon, but if the team ever wants to share state across machines (CI integration, remote dashboard), this becomes a hard wall.
- **No transaction abstraction**. `run_manager` may do multiple DB calls in a logical operation with no guarantee of atomicity.
- **Migration system is manual**: a `CURRENT_SCHEMA_VERSION` constant and a `run_migrations()` match statement. No `sqlx migrate` or `refinery`.

### 6.2 Event Bus

- **Broadcast capacity is 256**. A lagged consumer (e.g., a slow SSE client) will be dropped silently. The code logs `debug!` but there is no backpressure strategy.
- **Coalescence window is 250 ms fixed**. No adaptive backoff.

### 6.3 Cache Freshness Check Does Synchronous I/O

**File:** `crates/richter-daemon/src/run_manager.rs`

```rust
pub fn is_fresh(&self, max_age: Duration) -> bool {
    // ... time check ...
    for file_path in &self.changed_files {
        if let Ok(meta) = std::fs::metadata(file_path) {  // <-- blocking syscall
            if let Ok(mtime) = meta.modified() {
                // ...
            }
        }
    }
    true
}
```

Called from async contexts. On network filesystems or repos with thousands of changed files, this will block the Tokio runtime.

### 6.4 Mobile Gateway is a Monolith

**File:** `crates/richter-daemon/src/mobile_gateway.rs` (1,639 LOC)

Contains:
- TCP server setup
- Ed25519 signature verification
- Nonce tracking (replay protection)
- Token-bucket rate limiting
- Device registration
- Scope enforcement
- Axum routes
- TLS handling

This is essentially a second API surface crammed into one file. It should be its own module directory (`mobile/`) or even its own crate.

### 6.5 Process Supervisor lacks Sandboxing

**File:** `crates/richter-daemon/src/supervisor.rs`

- Commands are validated for length and forbidden characters, but there is **no sandbox** (no seccomp, no seatbelt, no chroot, no minimal privilege dropping).
- The supervisor uses `setpgid` and `killpg` for signal management — good — but a malicious plugin or hijacked agent could still exfiltrate or damage.
- **Dangerous env vars are blocked** (`PATH`, `LD_PRELOAD`, `DYLD_*`) but this is a deny-list, not an allow-list. New dangerous env vars require code changes.

### 6.6 CLI Shim Detection is Hard-Coded

**File:** `crates/richter-cli/src/main.rs`

```rust
let known_tools = [
    "cargo", "go", "npm", "npx", "yarn", "pnpm", "pip", "pip3", "python", ...
];
```

Only 21 tools. New build tools (e.g., `bun`, `zig`, `moon`) require a code change and recompile. This should be configurable.

### 6.7 Auth Token is Single, Static

- `generate_auth_token()` creates a SHA256 hash of random bytes and writes to a file with `0600`.
- No rotation. No revocation list. If the token is leaked (e.g., via an env dump), there is no recovery mechanism short of deleting the file.

---

## 7. 12–24 Month Velocity Forecast

| Horizon | Prediction |
|---------|------------|
| **0–3 months** | Fast. The code works, the ADRs are clear, and the monorepo makes cross-cutting changes easy. |
| **3–6 months** | Moderate. The importance pipeline needs real model integration. The `DaemonState` starts to hurt when adding new services (e.g., a metrics exporter). |
| **6–12 months** | Slow. `db.rs` and `models.rs` become merge-conflict factories. New engineers will fear touching `mobile_gateway.rs`. Plugin authors will ask for a real API and be disappointed. |
| **12–24 months** | Unless refactored, the architecture becomes a **tax**. Every new feature requires touching 3–4 mega-modules. Tests take longer because inline `#[cfg(test)]` modules bloat compile times. |

---

## 8. Concrete Evidence Index

| Finding | File(s) | Lines / Symbols |
|---------|---------|-----------------|
| God object `DaemonState` | `crates/richter-daemon/src/api.rs` | Lines 45–120 |
| Mega-models monolith | `crates/richter-core/src/models.rs` | 937 LOC |
| Mega-DB monolith | `crates/richter-core/src/db.rs` | 1,549 LOC |
| Mega-API monolith | `crates/richter-daemon/src/api.rs` | 1,061 LOC |
| Mega-mobile-gateway | `crates/richter-daemon/src/mobile_gateway.rs` | 1,639 LOC |
| Stub model boosts | `crates/richter-daemon/src/importance/pipeline.rs` | Lines 144–155 |
| No-op frontier boost | `crates/richter-daemon/src/importance/pipeline.rs` | Lines 157–160 |
| Cache freshness blocks | `crates/richter-daemon/src/run_manager.rs` | Lines 124–148 (`is_fresh`) |
| Plugin runtime stub | `crates/richter-daemon/src/plugin_runtime.rs` | ~90 LOC, no tests |
| Hard-coded shim list | `crates/richter-cli/src/main.rs` | Lines 219–236 |
| Inline tests only | All crates | No `tests/` directories; only `#[cfg(test)]` modules |
| Unidirectional deps | All `Cargo.toml` files | `richter-core` ← all others |
| Good trait abstractions | `crates/richter-daemon/src/mcp_bridge.rs` | `ToolHandler` trait |
| Good event bus | `crates/richter-daemon/src/event_bus.rs` | 390 LOC, well-tested |

---

## 9. Recommendations (Prioritized)

1. **Split `richter-core` into sub-crates or at least sub-modules:** `richter-db`, `richter-models`, `richter-config`. The current crate violates the Single Responsibility Principle.
2. **Extract mobile gateway into a `mobile/` directory** with `auth.rs`, `rate_limit.rs`, `nonce.rs`, `routes.rs`. 1,639 lines in one file is unsustainable.
3. **Replace `DaemonState` with a service locator or context object** that groups related handles behind domain-specific facades. Do not add a 13th field.
4. **Implement the importance pipeline stubs** or remove the ADR claims. Over-promising in architecture docs erodes trust.
5. **Move tests into `tests/` integration directories** for the daemon. Inline tests are fine for pure functions, but daemon behavior needs whole-system tests.
6. **Add a transaction wrapper to `db.rs`** so that multi-step operations (e.g., insert run + insert event) are atomic.
7. **Make the CLI shim list configurable** (e.g., `~/.richter/shims.toml`) so users do not need a recompile to add `zig` or `moon`.

---

## 10. Grade

| Dimension | Score | Notes |
|-----------|-------|-------|
| Crate Boundaries | B+ | Clean DAG, but `core` is too big. |
| Domain Separation | B | Good intent, `models.rs` and `db.rs` are monoliths. |
| Coupling / Cohesion | C+ | God object, raw SQL, tight CLI↔daemon JSON coupling. |
| Extensibility | B | Traits exist, but stubs and hard-coded lists limit growth. |
| Hidden Fragility | C+ | Blocking I/O in async, no sandbox, static auth token. |
| Long-term Velocity (12–24mo) | C+ | Accelerates now, will slow down without the refactor recommendations above. |

**Overall: Startup-grade with world-class aspirations. Needs a consolidation and modularization pass before the team scales past ~3 engineers.**
