# QA / Testing / Delivery Review — Richter

**Scope:** Unit, integration, concurrency tests; CI/CD; local dev experience; docs; release process; gaps.
**Analyzed at commit:** `a78c22d` (`main`)

---

## 1. Test Coverage Assessment

### Stats
| Metric | Count |
|---|---|
| Total Rust source lines (all crates) | ~22,661 |
| Integration test files (`crates/**/tests/*.rs`) | **8 files** (1,460 lines) |
| Files with `#[test]` attributes | **37** |
| Files with `#[tokio::test]` attributes | **10** |
| Top-level CLI smoke tests (`tests/cli_smoke.rs`) | **5 tests** |
| Explicitly `#[ignore]`ed tests | **2** (`crates/richter-daemon/tests/concurrent_agent_e2e.rs`) |
| Property-based / fuzz tests | **1** (`fingerprint.rs` via `proptest`) |
| XCTest / macOS app tests | **None found** |
| Mobile app tests | **None found** |
| Benchmark suite | **None** |
| Fuzzing / Miri | **None** |

### Coverage by Crate
| Crate | Lines | Test Modules Inline | Integration Tests |
|---|---|---|---|
| `richter-core` | ~6,766 | 14 files (`classifier`, `redact`, `models`, `config`, `git`, `db`, `fingerprint`, `resource`) | `tests/db_integration.rs` (19 tests) |
| `richter-daemon` | ~8,948 | 12 files (`mobile_gateway`, `event_bus`, `mcp_bridge`, `run_manager`, `scheduler`, `watcher`, `supervisor`, `importance/parsers`) | 6 integration files (API auth, event bus, run manager, scheduler, concurrency, concurrent E2E) |
| `richter-cli` | ~3,891 | 15 files (most commands) | `tests/cli_smoke.rs` |
| `richter-mcp` | ~3,056 | 4 files (`resources`, `transport`, `tools/handler`, `server/protocol`) | `tests/mcp_integration.rs` (8 tests) |

### Quality Notes
- **Strengths:**
  - `db_integration.rs`: 19 comprehensive SQLite CRUD tests covering runs, agents, repos, leases, settings, schema validation.
  - `event_bus_integration.rs`: 4 tests verifying pub/sub delivery, filtering, and serde round-trips.
  - `mcp_integration.rs`: 8 tests covering protocol handshake, tools/resources lists, error handling, and JSON-RPC id propagation.
  - `concurrency_stress.rs`: 4 tests (multi-thread scheduler, fingerprint determinism, concurrent `run_or_join`, cache-hit counters).
  - `fingerprint.rs` uses `proptest` for property-based testing (determinism, cwd sensitivity, arg sensitivity).
- **Known Flaws:**
  - Two critical E2E deduplication tests in `concurrent_agent_e2e.rs` are `#[ignore]`ed with comment: `"pre-existing race in run completion hook; fix requires async watch channel"`.
  - Most CLI command tests are trivial (e.g., assert output is non-empty or command requires args). No behavioral assertions for `run`, `install`, `setup`, `claim`, or `audit`.
  - No tests for the SwiftUI macOS app or the mobile companion app.
  - No load/perf benchmarks for the scheduler, classifier, or fingerprint engine.
  - No fuzzing or miri checks for unsafe-adjacent code (the `unsafe` count is small, but not zero).
  - No migration tests — the SQLite schema is bootstrapped inline in `db.rs`. There is no migration framework (e.g., `refinery`, `sqlx migrate`).

---

## 2. CI/CD Maturity

### GitHub Actions (`.github/workflows/ci.yml`)
Five jobs run on **macos-latest** only:

1. **Check & Lint** — `cargo fmt --check`, `cargo clippy -D warnings`, `cargo check --workspace --all-features`
2. **Security Audit** — `cargo install cargo-audit && cargo audit`
3. **License Check** — `cargo install cargo-deny && cargo deny check`
4. **Test** — `cargo test --workspace --all-features` (depends on check + audit)
5. **Build All Targets** — `cargo build --workspace --all-targets --all-features` (depends on check)

### Maturity Score: **C+**
- **Pros:**
  - Security advisories (`cargo audit`) and license compliance (`cargo deny`) are enforced.
  - Rust cache (`Swatinem/rust-cache@v2`) is used.
  - Build/test are separate; formatting and clippy gating is correct.
- **Cons / Gaps:**
  - **Single-platform CI:** Only macOS runners. No Linux or Windows verification. If this ever targets cross-platform, you have zero signal.
  - **No timeout on jobs or steps** — the default GHA runner timeout is 6 hours; a hung test can waste resources.
  - **No artifact / release pipeline** — No `.github/workflows/release.yml`, no automated binary builds, no app bundle distribution.
  - **No code coverage reporting** — `CONTRIBUTING.md` mentions `cargo-llvm-cov`, but CI does not run or upload coverage.
  - **No matrix builds** across Rust versions (only `stable`).
  - **No integration-test-specific runner** — all tests run in the same `cargo test` invocation, which means if the daemon leaves child processes or temp files behind, subsequent jobs could be affected.
  - **`cargo audit` and `cargo deny` reinstall every run** — caching these binaries would shave 1–3 minutes.

---

## 3. Local Developer Experience Quality

### Setup
One-liner builds and tests work: `cargo build`, `cargo test --workspace`, `make check`.

### Scripts
| Script | Purpose | Quality |
|---|---|---|
| `scripts/build.sh` | Release/debug; rust-only; app-only | Good — handles both Rust and Xcode builds |
| `scripts/test.sh` | Wrapper around `cargo test`; supports `--nextest`, `--crate` | Good — includes prerequisite checks, colored output, failure counting |
| `scripts/install.sh` | Full install: binaries, LaunchAgent, shell shims, MCP configs | Excellent — supports `--yes`, auto-detects shell, uninstall instructions |
| `scripts/demo.sh` | Walkthrough demo | Not reviewed in depth |
| `scripts/e2e_mobile_test.sh` | Mobile endpoint smoke test via `curl` | Functional but requires manual daemon start |
| `scripts/run_simulation.sh` | Sim workload | Simple |

### Tooling
- `Makefile`: clean, well-organized targets (`build`, `test`, `lint`, `fmt`, `check`, `run`, `install`).
- `deny.toml`: reasonable license allow-list; wildcards denied.
- `Cargo.toml` workspace: resolver "2", shared deps, `rust-version = "1.80"`.
- **Missing:**
  - No `Dockerfile` / devcontainer for reproducible environments.
  - No `justfile` / `mise.toml`.
  - No pre-commit hook configuration.
  - No `cargo-nextest` adoption in CI or Makefile (test script supports it, but Makefile does not).

---

## 4. Documentation Completeness

### Score: **A-**
Richter is unusually well-documented for a young project:

- **8 ADRs** covering architecture, fingerprints, endpoint security, LLM importance pipeline, mobile companion, pairing/trust model, React Native choice, local-first remote access.
- **16 markdown docs**
  - `ARCHITECTURE.md` — system overview, crate map, data flow diagrams.
  - `INSTALL.md` — step-by-step build/install.
  - `USER_GUIDE.md` — day-to-day usage.
  - `SECURITY.md` / `PRIVACY.md` / `MOBILE_SECURITY.md` — threat model, keychain usage, pairing flow.
  - `TROUBLESHOOTING.md` — common issues.
  - `MODELS.md`, `INTEGRATIONS.md`, `MOBILE.md`, `REMOTE_ACCESS.md`, `NOTIFICATIONS.md`.
  - `ARCHITECTURE_REVIEW.md`, `PERFORMANCE_SCALABILITY_REVIEW.md`, `RELIABILITY_OPS_REVIEW.md`, `TOTAL_MEMORY_MCP_PLAN.md`.
- `CONTRIBUTING.md` — clear crate map, setup, test strategy, style rules, CI summary.
- `README.md` — concise, includes quick-start and architecture block diagram.

- **Minor gaps:**
  - No `CHANGELOG.md` (makes release tracking hard).
  - `CONTRIBUTING.md` mentions `cargo-llvm-cov` for coverage, but running it is not in CI and not in `scripts/test.sh`.
  - No specific docs for new contributors on how to add a DB migration.

---

## 5. Release / Deployment Process Maturity

### Score: **D**

- **No automated release workflow** in `.github/workflows/`.
- **No `CHANGELOG.md`**, no release notes, no Git tag automation.
- Workspace version is pinned at `0.1.0` everywhere.
- No `cargo publish` / crates.io publication pipeline.
- No Homebrew formula, no `.dmg` / `.pkg` builder, no app notarization workflow.
- No release branch strategy or versioning policy documented.

In short: shipping currently requires manual building and distributing binaries. For a macOS-only LaunchAgent-based tool, this is acceptable while in alpha, but it is a hard blocker for any public distribution.

---

## 6. What Would Prevent Confident Shipping

### 🔴 Critical
1. **Ignored E2E deduplication tests** — the core value proposition ("two agents join the same run") is untested in CI because the tests are skipped. A regression here would break the product and no one would know.
2. **No automated release pipeline** — manual binary distribution is error-prone and cannot be reproduced or audited.
3. **No migration framework** — SQLite schema changes require manual SQL migration. A schema mismatch between versions will cause runtime crashes or data loss.

### 🟡 High
4. **Zero macOS app unit tests** — The SwiftUI app (`apps/macos/RichterApp`) has no XCTest targets. UI regressions in setup, settings, or the dashboard would go unnoticed.
5. **No mobile test coverage** — The mobile companion logic (pairing, trust model, remote access) relies entirely on Rust unit tests in `mobile_gateway.rs` and manual `curl` scripts. No XCTest or appium tests.
6. **Single-platform CI** — If any cross-platform aspirations exist (or even cross-version macOS stability), the signal is missing. More importantly, CI does not exercise the actual daemon lifecycle (install, start via launchctl, stop, uninstall).
7. **No code coverage gating** — `CONTRIBUTING.md` mentions it but CI does not run it. There is no visibility into which modules are undertested.
8. **No database WAL checkpoint / busy timeout retry** — Noted in `docs/RELIABILITY_OPS_REVIEW.md`: a single DB connection, no retry on `SQLITE_BUSY`, hot-path cache lookups block.

### 🟢 Medium
9. **Performance: no benchmarks** — The classifier runs on every intercepted command. No benchmark means regressions in dedup latency or classification overhead are invisible.
10. **No fuzz / property tests beyond fingerprint** — The API surface (HTTP + MCP + socket) and the event bus are prime candidates.
11. **No security regression suite** — Auth bypass attempts, malformed MCP envelopes, path-traversal edge cases, and replay attacks are tested ad-hoc but not systematically.
12. **Missing `unsafe` audit** — There are a few `unsafe` usages (low count). A pass with `cargo miri test` would improve confidence.

---

## 7. Gaps in Testing (by Category)

| Category | Status | Details |
|---|---|---|
| **Unit tests** | ✅ Good | Core data types, classifier, redaction, DB, models all have inline tests. |
| **Integration tests** | ✅ Good | DB, API auth, event bus, MCP protocol, scheduler, run manager all covered. |
| **Concurrency / Race** | ⚠️ Partial | Stress tests and scheduler deadlock tests exist, but **2 E2E dedup tests ignored**. |
| **Crashes / Panics** | ❌ Missing | No `no_panic` or `std::panic::catch_unwind` harness in CI. No `abort` recovery tests. |
| **Security** | ⚠️ Partial | Auth tests present (API 401s); mobile crypto (Ed25519, nonces) has unit tests. No systematic security regression suite. |
| **Performance / Throughput** | ❌ Missing | No benchmarks for classifier, fingerprint, scheduler, or event bus throughput. |
| **macOS App** | ❌ Missing | No XCTest targets; UI is untested. |
| **Mobile Companion** | ❌ Missing | Mobile endpoints tested only via manual `curl` script. |
| **End-to-End Daemon Lifecycle** | ❌ Missing | No tests that install, start, interact with, and uninstall the daemon via LaunchAgent. |

---

## 8. Recommendations (Ranked by Impact)

1. **Un-ignore or replace the 2 E2E deduplication tests.** Fix the async watch-channel race and add these to CI. They validate the core value proposition.
2. **Add a `release.yml` workflow.** Build release binaries, codesign the `.app` if possible, generate release notes, push a Git tag.
3. **Introduce a migration framework** (`sqlx migrate` or `refinery`) and add a migration test verifying `N-1` → `N` schema upgrades.
4. **Run coverage in CI** (`cargo-llvm-cov` → upload to codecov). Gating on coverage regressions forces attention to undertested modules.
5. **Add macOS app XCTest target.** Even a handful of tests verifying `DaemonClient` JSON decoding and view-model state transitions would catch real regressions.
6. **Add a daemon-lifecycle integration test** that exercises the full install/start/status/stop/uninstall flow on a macOS runner.
7. **Add `timeout-minutes:** 30` to CI jobs so hung tests or builds fail fast.
8. **Cache `cargo-audit` and `cargo-deny`** binaries in CI to reduce redundant installs.
9. **Add property-based tests for the classifier** (similar to `fingerprint.rs`) to catch parser regressions.
10. **Set up one benchmark** (e.g., `criterion` for classifier + fingerprint throughput) and run it in CI with `bencher.dev` or similar to detect regressions.

---

## 9. Bottom Line

Richter has a **solid testing foundation** for a pre-1.0 project: good crate-level unit tests, a healthy set of integration tests for the DB and MCP surfaces, and even property-based testing. The developer experience is polished (excellent install script, good docs, clear CONTRIBUTING guide).

However, the project is **not yet ready for confident shipping** because:
- Its **most important feature** (run deduplication) has ignored E2E tests.
- There is **zero release automation**.
- **macOS app and mobile companion** have no automated test coverage.
- **Database migration** is manual and fragile.

Fix the dedup tests and add a release workflow, and confidence rises dramatically. Add app tests and migration safety, and this becomes a production-grade delivery story.
