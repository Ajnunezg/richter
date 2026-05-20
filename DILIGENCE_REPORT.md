# Richter Technical Diligence Report

**Date:** 2026-05-17
**Scope:** Full repository — 4 Rust crates (~22,700 LOC), SwiftUI macOS app (~2,300 LOC), CI, docs, ADRs, scripts
**Methodology:** Multi-agent swarm deep-dive (architecture, code quality, reliability/ops, security, performance, QA/testing)
**Repo:** `/Users/dewclaw/Documents/Projects/Richter`
**Commit:** `a78c22d` ("feat: implement remediation.md end-to-end")

---

## 1. Executive Summary

**Richter is a well-above-average solo-developer codebase with genuine technical ambition, but it is not ready for production launch and would not survive Series A technical diligence without several months of hardening.**

The architecture is coherent, the Rust quality is real, and the documentation (especially the 8 ADRs) is excellent. The developer clearly understands async systems programming, macOS-native constraints, and the problem domain. This is not a hacky prototype — it's an ambitious, imperfect startup codebase with clear evidence of engineering taste.

However, there are material gaps across every dimension: the security posture is missing operational controls (no TLS, no database encryption, no rate limiting), the reliability story lacks end-to-end testing and monitoring, the performance implementation has a memory leak and CPU waste, and the architecture has mega-modules that will resist refactoring as the team grows.

**Bottom line:** This would impress a strong engineer as "promising" but alarm a technical diligence partner as "not yet production-grade."

---

## 2. Final Verdict

| Question | Answer |
|----------|--------|
| Is this a hacky prototype? | **No.** It's too well-structured, documented, and tested for that label. |
| Is this a credible production system? | **Not yet.** Missing too many operational controls. |
| Would a Series A investor be impressed? | **Impressed by the vision and ADRs. Nervous about the gaps.** |
| Can this launch tomorrow? | **No.** Several launch blockers exist (see §6). |
| Can it launch with caveats? | **Yes** — if the team accepts known risks and commits to hardening within 90 days. |
| Is it Series A diligence-ready? | **No.** Would fail on security, reliability, and testing gaps. |

**Overall Verdict: Launchable with major caveats. Not diligence-ready. Strong solo-developer foundation trending toward startup-grade.**

---

## 3. Scorecard

| Category | Score (1-10) | Rationale | Severity |
|----------|:-----------:|-----------|----------|
| **Architecture** | 7 | Clean crate DAG, good domain types, excellent ADRs. Undermined by mega-modules (db.rs 1,548 LOC, mobile_gateway.rs 1,638 LOC), a 13-field god object (DaemonState), and thin trait abstractions. | Medium |
| **Code Quality** | 7 | Well-typed domain model, exhaustive enums, correct async patterns. Marred by raw HTTP/1.1 string-building in two clients, code duplication between `mcp_bridge` and `richter-mcp`, and 213 `.clone()` calls across hot paths. | Medium |
| **Reliability / Ops** | 6 | Structured tracing, 30s graceful shutdown, process supervision with stall detection. No end-to-end tests, no metrics, no config hot-reload, no binary signing, no crash-loop detection in LaunchAgent. | High |
| **Security** | 5 | Constant-time auth, 0600 permissions, secrets redaction engine, Ed25519 mobile auth. **No TLS** despite docs, no database at-rest encryption, no daemon API rate limiting, webhook secrets stored but never verified, stale Cargo.lock with vulnerable deps. | **Critical** |
| **Performance / Scalability** | 6 | Correct async primitives, semaphore concurrency, two-tier cache. `System::new_all()` created twice per poll (CPU waste), completed runs map unbounded (memory leak), blocking I/O in async paths, static concurrency limit of 6. Zero benchmarks. | High |
| **Testing / CI / Delivery** | 5 | 204 test functions, CI with clippy + fmt + cargo-audit + cargo-deny. Zero integration tests, zero SwiftUI tests, zero benchmarks, zero fuzz tests, macOS-only CI, no release process, no pre-commit hooks. | **Critical** |
| **Documentation / Maintainability** | 7 | Excellent docs (ARCHITECTURE.md, 8 ADRs, USER_GUIDE, INSTALL, INTEGRATIONS, SECURITY, MOBILE). Some docs over-promise relative to implementation (TLS claims, trait abstractions claimed). Good CONTRIBUTING.md. | Low |
| **Overall Professionalism** | 6.5 | A credible solo-developer effort with strong fundamentals. Not yet a team-scale production system. | — |
| **Launch Readiness** | 5 | Launch blockers exist in security and testing. Could launch for internal/alpha use, not for general availability. | — |
| **Series A Diligence Readiness** | 4 | Would fail on multiple diligence dimensions. Needs 2-3 months of focused hardening. | — |

**Overall Weighted Score: 61 / 100**

**Confidence Level in Assessment: HIGH** — Based on comprehensive multi-agent review of ~25,000 LOC, all CI/config/docs files, and systematic verification of key claims.

---

## 4. What Inspires Confidence

### 4.1 Architecture Decision Records (ADRs)
The 8 ADRs (`adr/0001-architecture.md` through `adr/0008-*.md`) are exceptional. They capture architectural rationale with clear context, tradeoffs considered, and consequences. This is rare even in Series A companies and signals genuine engineering maturity.

### 4.2 Clean Crate Dependency Graph
```
richter-core ← richter-daemon
             ← richter-cli
             ← richter-mcp
```
No cycles. Unidirectional. Correct layering. The `richter-core` crate is a true library with no awareness of daemon/CLI/MCP. This discipline is hard to maintain and the developer held the line.

### 4.3 Event Bus Design
`crates/richter-daemon/src/event_bus.rs` (437 LOC) is the best module in the repo. One `DaemonEvent` enum, one bus with typed emit/subscribe, event coalescence with 250ms window, and well-tested. This is the kind of focused, cohesive module that signals good taste.

### 4.4 Command Classifier
`crates/richter-core/src/classifier.rs` (866 LOC) covers 7+ ecosystems (JS, Python, Rust, Go, Swift, Java, Bazel) with well-structured pattern matching. It's deterministic and isolated from the scheduler. This level of domain modeling is above average.

### 4.5 Secrets Redaction Engine
`crates/richter-core/src/redact.rs` (313 LOC) detects API keys, bearer tokens, private keys, GitHub/OpenAI/Anthropic/DeepSeek/AWS/GCP/Azure credentials with conservative regex patterns. It errs on the side of over-redaction. This shows security-conscious design thinking.

### 4.6 Async Correctness
No locks held across `.await` points found anywhere in the codebase. `parking_lot::Mutex` used for sync paths, `tokio::sync::Semaphore` for async concurrency control, `dashmap::DashMap` for concurrent maps. This is the kind of discipline that prevents the hardest class of async bugs.

### 4.7 Configuration Layering
Config loading with `RichterConfig::load()` that merges baked-in defaults, `~/.richter/config.toml`, and env vars. Deserialized via serde with `#[serde(default)]` for forward compatibility. Correct pattern.

---

## 5. What Would Alarm a Serious Reviewer

### 5.1 No Database Encryption at Rest
**File:** `crates/richter-core/src/db.rs`
**Risk:** The SQLite database stores command outputs, cache entries, and system state with **0600 file permissions as the only protection**. There is no SQLCipher, no file-level encryption, no key derivation. If a developer's machine is compromised or a backup is leaked, all Richter state is readable. For a tool that coordinates multiple AI agents and can see command output, this is a material data exposure risk.

### 5.2 Mobile Gateway TLS is Unimplemented Despite Docs
**File:** `crates/richter-daemon/src/mobile_gateway.rs`
**Evidence:** `MobileGatewayConfig` has a `_use_tls` field that is never read. The docs (MOBILE.md, MOBILE_SECURITY.md) claim TLS is "on by default." The actual TCP server uses plaintext. This is a documentation-to-code gap that would be caught in diligence.

### 5.3 Unbounded Memory Growth (Completed Runs Map)
**File:** `crates/richter-daemon/src/supervisor.rs:108`
**Risk:** `completed: Arc<DashMap<String, CompletedChild>>` has no TTL, no max-size cap, no eviction logic. Each `CompletedChild` holds up to 1MB of output. At ~1000 runs/day, this leaks ~1GB/day. This is a memory leak that **will** crash the daemon under sustained use.

### 5.4 CPU Waste from sysinfo Misuse
**File:** `crates/richter-daemon/src/scheduler.rs:30-37`
**Risk:** `ResourceMonitor::poll()` creates a new `System::new_all()` on **every invocation**. This is expensive — it walks all processes on the system. The method also creates a *second* `System` that's never reused. This burns measurable CPU in the daemon's hot loop. A prior performance review (May 5) flagged this and it has not been fixed.

### 5.5 Zero Integration or End-to-End Tests
**Evidence:** Zero `tests/` directories exist in any crate. All 204 test functions are inline `#[cfg(test)]` modules. The core value proposition — "run-or-join prevents duplicate work" — has no end-to-end test. Two ignored tests exist in `richter-daemon/src/e2e_tests.rs` that attempt to test this but are disabled. Without e2e tests, the system's primary behavior is untested.

### 5.6 God Object Anti-Pattern
**File:** `crates/richter-daemon/src/api.rs:45-120`
**DaemonState** has 13 fields: event bus, run manager, scheduler, supervisor, token path, auth token, repos, settings, install status, mobile state, model call budget, database, and watcher health. Every API handler depends on all of this. This is the exact pattern that turns a 1,000-line file into a 3,000-line file as features are added.

### 5.7 MongoDB-Sized Module Files
- `mobile_gateway.rs`: 1,638 LOC (TLS, auth, pairing, rate limiting, routes in one file)
- `db.rs`: 1,548 LOC (schema, migrations, 30+ CRUD methods, no repository pattern)
- `api.rs`: 1,060 LOC (routing, auth middleware, SSE, mobile forwarding)
- `run_manager.rs`: 969 LOC (cache, fingerprinting, superset detection, join logic)

These will become merge-conflict factories and onboarding barriers as the team grows beyond 1-2 engineers.

### 5.8 Raw HTTP/1.1 String Construction
**Files:** `crates/richter-cli/src/client.rs`, `apps/macos/RichterApp/.../DaemonClient.swift`
Both the CLI client and the SwiftUI app construct HTTP/1.1 requests via **string concatenation**. No HTTP library. This is fragile (headers, encoding, chunking) and would fail basic code review at any funded company.

---

## 6. Launch Blockers

These should be fixed before any production launch:

| # | Issue | File(s) | Impact |
|---|-------|---------|--------|
| LB-1 | **Completed runs memory leak** — unbounded DashMap growth | `supervisor.rs:108` | Daemon crash under sustained use |
| LB-2 | **CPU waste from double System::new_all()** | `scheduler.rs:30-37` | Battery drain, CPU hotspots |
| LB-3 | **Zero end-to-end tests** — core run-or-join untested | All crates | No confidence primary behavior works |
| LB-4 | **No TLS on mobile gateway** — docs claim it's on | `mobile_gateway.rs` | Data exposure over network |
| LB-5 | **Stale Cargo.lock with vulnerable deps** | `Cargo.lock` | Known-vulnerable regex, nix versions |
| LB-6 | **No auth failure logging** — silent 401s | `api.rs` | Cannot detect or respond to attacks |
| LB-7 | **Webhook secrets not verified** — stored but no HMAC check | `webhooks.rs` | Fake webhooks accepted |

---

## 7. Diligence Risks

Issues that would surface in Series A technical diligence:

| # | Risk | Why It Matters to Investors |
|---|------|----------------------------|
| DR-1 | **No database encryption at rest** | Data protection diligence question. "How do you protect agent output data?" |
| DR-2 | **No rate limiting on daemon API** | DoS surface. "Can a malicious agent flood the daemon?" |
| DR-3 | **Single static auth token, no rotation** | "What happens when the token leaks?" |
| DR-4 | **No binary signing/notarization** | macOS gatekeeper will block installs. "How do users install safely?" |
| DR-5 | **macOS-only CI** | "Do you test on Linux? On Apple Silicon vs Intel?" |
| DR-6 | **No release process** | "How do you ship? What's the versioning strategy?" |
| DR-7 | **Zero production metrics** | "How do you know it's working in production?" |
| DR-8 | **Plugin runtime is a 90-line stub** | "The architecture docs mention plugins. Is this real?" |
| DR-9 | **Importance engine stubs contradict ADR-0004** | "Your ADR describes a sophisticated LLM pipeline. Where is it?" |
| DR-10 | **Hard-coded shim list (21 tools)** | "How do users add support for zig, moon, or their own tools?" |

---

## 8. Hidden Rewrite Risks

| Area | Risk Level | Why |
|------|:----------:|-----|
| **Database layer** | Medium | `db.rs` is a 1,548-line monolith with raw SQL and no repository pattern. If the team needs to support PostgreSQL, add multi-machine state, or change schema significantly, a rewrite of this module is likely. |
| **Mobile gateway** | Medium-High | 1,638-line monolith with stubbed TLS. If mobile becomes a real product, this file needs to be split into `mobile/` sub-modules with proper auth, pairing, and sync primitives. |
| **MCP integration** | Medium | `mcp_bridge.rs` in daemon duplicates tool definitions from `richter-mcp` crate. A docstring says it "will eventually" be consolidated. When this consolidation happens, both crates need significant refactoring. |
| **Plugin system** | High | 90-line stub with no sandbox, no IPC, no capability enforcement, and no tests. If plugins are a real feature, this is a full rewrite. |
| **Importance engine** | Medium | `frontier_model_boost` is a no-op. The LLM pipeline described in ADR-0004 is mostly unimplemented. Real model integration will require substantial new code. |
| **SwiftUI app** | Low-Medium | 2,300 LOC with raw HTTP client. If the app becomes a primary interface, the networking layer needs a rewrite (URLSession, structured models). |

**Overall rewrite risk: Medium.** The core domain model and fingerprinting are solid. The main risks are in peripheral systems that are currently stubs or early implementations.

---

## 9. Top 10 Highest-Leverage Improvements

Ranked by impact on professionalism and readiness:

1. **Add TTL eviction to the completed runs map** — Fixes a memory leak that will crash the daemon. 4 lines of code. Highest impact-to-effort ratio.
2. **Fix the double System::new_all() in scheduler** — Replace with `sys.refresh_cpu()` / `sys.refresh_memory()` on a persistent System. 8 lines of code. Major CPU savings.
3. **Implement end-to-end tests for run-or-join** — The core value prop is untested. Enable the 2 ignored e2e tests and add 3-5 more covering cache hits, deduplication, and resource backpressure.
4. **Enable TLS on the mobile gateway** — Or remove the TLS claims from documentation. The current docs-to-code gap is a diligence red flag.
5. **Add database encryption at rest** — Integrate SQLCipher or at minimum document why plain SQLite is acceptable for the threat model. File permissions alone are insufficient.
6. **Add rate limiting to the daemon API** — Even a simple token-bucket per IP/socket would close the DoS surface. The mobile gateway already has rate limiting; apply the same pattern.
7. **Split mega-modules into sub-modules** — Start with `mobile_gateway.rs` → `mobile/` (auth, pairing, rate_limit, routes) and `db.rs` → `db/` (schema, migrations, repositories per table).
8. **Replace DaemonState with a service locator** — Group related handles behind domain-specific facades. Do not add a 14th field to DaemonState.
9. **Add Prometheus metrics and latency histograms** — Wrap `run_or_join` with `tracing::info_span!` and export structured metrics. This enables production debugging and investor confidence.
10. **Make the CLI shim list configurable** — `~/.richter/shims.toml` so users can add `zig`, `moon`, `bun` without recompiling.

---

## 10. Appendix: Evidence Index

### Files Inspected (representative sample)

| File | LOC | Assessment |
|------|-----|------------|
| `crates/richter-core/src/db.rs` | 1,548 | Monolithic; needs repository pattern |
| `crates/richter-core/src/models.rs` | 936 | Monolithic type definitions; needs splitting |
| `crates/richter-core/src/classifier.rs` | 866 | Well-structured ecosystem classifier |
| `crates/richter-core/src/config.rs` | 731 | Clean serde config, good layering |
| `crates/richter-core/src/resource.rs` | 756 | Resource class definitions |
| `crates/richter-core/src/redact.rs` | 313 | Strong secrets detection engine |
| `crates/richter-core/src/fingerprint.rs` | ~400 | BLAKE3 + git state, solid |
| `crates/richter-daemon/src/api.rs` | 1,060 | God object, mixed concerns |
| `crates/richter-daemon/src/mobile_gateway.rs` | 1,638 | Monolith; TLS stubbed |
| `crates/richter-daemon/src/run_manager.rs` | 969 | Core orchestration; sync I/O in async |
| `crates/richter-daemon/src/scheduler.rs` | 524 | sysinfo misuse; static limits |
| `crates/richter-daemon/src/supervisor.rs` | 758 | Memory leak in completed map |
| `crates/richter-daemon/src/watcher.rs` | 522 | blocking_send in FSEvent callback |
| `crates/richter-daemon/src/event_bus.rs` | 437 | **Best module** — focused, well-tested |
| `crates/richter-daemon/src/importance/pipeline.rs` | 183 | frontier boost is no-op stub |
| `crates/richter-daemon/src/mcp_bridge.rs` | 396 | Duplicates richter-mcp tool defs |
| `crates/richter-cli/src/main.rs` | ~300 | Hard-coded shim list (21 tools) |
| `crates/richter-cli/src/client.rs` | ~200 | Raw HTTP/1.1 string building |
| `.github/workflows/ci.yml` | 54 | macOS-only, no integration tests |
| `Makefile` | 26 | Minimal; adequate for solo dev |
| `scripts/install.sh` | ~300 | Comprehensive install script |
| `adr/0001-architecture.md` | — | Excellent architectural rationale |
| `apps/macos/.../DaemonClient.swift` | ~200 | Raw HTTP/1.1 over NWConnection |

### Test Statistics (from grep analysis)
- 37 files contain test code
- 204 test functions
- 41 `#[cfg(test)]` modules
- **0** integration test directories (`tests/`)
- **0** SwiftUI/XCTest tests
- **2** ignored e2e tests
- **0** benchmarks
- **3** property-based tests (proptest)

### Notable Patterns
- **Good:** No locks held across `.await` anywhere — confirmed by comprehensive sweep
- **Good:** `parking_lot::Mutex` used instead of `std::sync::Mutex` throughout
- **Good:** `dashmap::DashMap` used for concurrent hash maps
- **Good:** `tokio::sync::watch` for event-driven completion signaling
- **Concerning:** 213 `.clone()` calls across the codebase
- **Concerning:** `anyhow::Result` used throughout daemon instead of structured error types
- **Concerning:** Blocking filesystem I/O (`std::fs::metadata`, `std::fs::read`) in async contexts

### Documentation Quality
- **Excellent:** ARCHITECTURE.md, all 8 ADRs, SECURITY.md, PRIVACY.md
- **Good:** USER_GUIDE.md, INSTALL.md, INTEGRATIONS.md, TROUBLESHOOTING.md
- **Problematic:** MOBILE.md claims TLS is "on by default" — it is not
- **Problematic:** ARCHITECTURE.md claims trait-based design for DB — it is concrete

---

*Report generated by a coordinated multi-agent swarm review. Architecture, code quality, reliability, security, performance, and QA/testing reviews were performed independently and merged into this consolidated assessment.*
