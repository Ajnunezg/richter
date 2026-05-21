# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

### Added
- **Security**: `SecretStringConfig` type — API keys for model providers are now redacted in Debug and never serialized to config files.
- **Architecture**: `HTTPModelBoost::from_config()` enables file-driven model provider setup (alternative to raw env vars).
- **Architecture**: `ErrorCode` enum with 11 machine-readable variants for programmatic API error handling.
- **Architecture**: `DaemonError` extended with `Fingerprint`, `CachePoisoned`, `SchedulerUnavailable`, `SpawnFailed`, and `InvalidCommand` variants.
- **CLI**: Typed HTTP client with `httparse` validation — status code checking, Content-Length verification, chunked-transfer support, and header-injection guards.
- **CLI**: `LocalClient::get()` and `LocalClient::post()` type-safe methods.
- **CI/CD**: Release workflow now creates a `richter-darwin-arm64.tar.gz` artifact and uploads it as a GitHub Actions artifact before creating the release.

### Fixed
- Security: Mobile gateway `approve_handler` / `deny_handler` now records the authenticated `DeviceId` in the audit trail instead of hardcoding `"daemon"`.
- Security: API error responses include structured `{"code": "...", "error": "...", "status": N}` instead of opaque strings.
- Security: CLI raw HTTP/1.1 string building replaced with byte-based `httparse` response handling.

### Changed
- `ImportanceConfig` now accepts `model_providers: Vec<ModelProviderConfig>` and `fallback_to_env: bool` for hybrid env/config discovery.

## [Unreleased]

### Added
- **Reliability**: Daemon health watchdog — periodic DB liveness check every 60s (configurable via `RICHTER_WATCHDOG_INTERVAL_SECS`).
- **Performance**: Fingerprint result caching — 5-second TTL cache avoids redundant git process spawns during multi-agent bursts (256 entry max).
- **Security**: Per-client rate limiting — rate limiter distinguishes between mobile devices (`X-Device-ID`) and MCP agents (`X-Agent-ID`) instead of a single static bucket.
- **Operations**: `docs/RUNBOOK.md` — comprehensive operational runbook covering daemon lifecycle, troubleshooting, monitoring, and data retention.
- **Security**: `docs/THREAT_MODEL.md` — STRIDE-based threat model for the daemon, CLI, MCP server, and mobile gateway.
- **Testing**: `tests/daemon_e2e.rs` — end-to-end integration tests for daemon startup, health checks, PID file double-start prevention, and cleanup.

### Changed
- **Security**: Database encryption status honestly reports `"key-managed (vfs-pending)"` instead of falsely claiming `"aes-256-gcm"`. Crypto primitives are ready but SQLite file is not encrypted at rest.
- **Architecture**: `db.rs` (1,700 LOC) decomposed into `db/mod.rs`, `db/rows.rs`, and `db/migrations.rs`.
- **Reliability**: `ErrorCode` enum derives `PartialEq` for test assertions.
- **Reliability**: Daemon logs encryption check as `warn` instead of `error` (non-fatal).
- **Style**: Workspace-wide `cargo fmt` pass.

### Added (prior)
- Strong ID types: all ID types (`RepoId`, `RunId`, `EventId`, etc.) are now newtype wrappers around `Uuid`, preventing accidental cross-type usage.
- Request timeout middleware on API (30s default).
- Request ID (`X-Request-Id`) propagation via tracing spans.
- Pre-migration database backup before schema changes.
- Atomic schema version tracking in SQLite migrations.
- TLS foundation for mobile gateway (`MobileTlsConfig` struct, self-signed cert generation stub).
- Configurable CLI shim list via `~/.richter/shims.toml`.
- `ModelBoostProvider` trait for pluggable importance pipeline backend.
- TTL eviction for completed runs in `ProcessSupervisor` (30-minute default).
- Background eviction task for completed run cleanup.
- `HashSet`-based `NonceTracker` for O(1) replay protection lookups.
- CI timeout limits, concurrency groups, lock file check, and scheduled weekly runs.

### Fixed
- Security: `redact()` is now wired into `supervisor::append_output()` and `run_manager` completion hooks. Secrets from build/test output are no longer persisted in plaintext.
- Security: Shell metacharacter injection mitigated — `sh -c` commands are validated via `shlex` round-trip and pattern blocklist.
- Security: Auth token is now regenerated on every daemon start (no silent reuse).
- Security: Bearer token no longer bypasses Ed25519 device auth on mobile gateway non-pairing endpoints.
- Performance: `ResourceMonitor::poll()` now reuses the `System` instance instead of creating `System::new_all()` on every call.
- Performance: `is_fresh()` and git-diff completion hook moved to `spawn_blocking` to avoid blocking the async runtime.
- Performance: `NonceTracker` uses `HashSet` instead of `Vec` for O(1) lookups.
- Reliability: Non-atomic schema version DELETE+INSERT replaced with `INSERT OR REPLACE` inside explicit transactions.
- Reliability: `ProcessSupervisor::completed` DashMap now has TTL eviction (30-minute default) to prevent memory leak.
- Reliability: `ProcessSupervisor::check_orphans()` is now called on daemon startup.
- Testing: Previously-ignored E2E deduplication tests are now enabled and passing.
- CLI: `test.sh` `local` keyword bug fixed (moved inside function).

### Changed
- `DaemonState` decomposed into `AppState` with domain-specific state groups (`RunState`, `SystemState`, `AuthState`).
- Mobile gateway split from single `mobile_gateway.rs` (1,639 LOC) into `mobile/` sub-module directory.
- ID types are now newtype wrappers (`struct RepoId(pub Uuid)`) instead of type aliases (`type RepoId = Uuid`).
- `DeviceId` newtype wrapper added for mobile device identifiers.
- Session tokens added for mobile device authentication after pairing.

### Removed
- Dead `resource.rs` module (756 LOC of unused `ResourceScheduler`/`ResourceManager`).
- `once_cell` dependency (replaced by `std::sync::LazyLock` which is stable in Rust 1.80+).
- Plaintext Bearer token acceptance on mobile gateway authenticated endpoints.
