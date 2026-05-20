# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
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
