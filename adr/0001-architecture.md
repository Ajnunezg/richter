# ADR 0001: System Architecture

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter is a local agent-control plane for macOS. It must coordinate multiple
concurrent AI coding agents (Codex, Claude Code, Droid, Forge Code, Kimi,
MiniMax, and future agents) that independently run build, test, lint, and
other developer commands. The system must:

- Prevent duplicate work (two agents running the same test suite simultaneously).
- Manage CPU and memory resources across concurrent heavy builds.
- Surface important events without flooding the user.
- Work without root privileges.
- Work without any cloud dependencies by default.
- Run on macOS 14+ (Apple Silicon and Intel).
- Integrate with agents via shell shims, MCP, and agent hooks.
- Provide a polished native macOS user experience.

We need to decide: what is the overall system architecture, what are the
components, how do they communicate, and what are the key technology choices?

## Decision

Richter uses a **four-component architecture**:

1. **Richter App** — SwiftUI macOS app (menu bar + dashboard window). The
   primary user-facing surface.
2. **Richter Daemon** (`richterd`) — Rust background service (SMAppService
   LoginItem). The orchestration engine owning all core logic.
3. **Richter CLI** (`richter`) — Rust terminal binary for direct interaction.
4. **Richter MCP** (`richter-mcp`) — MCP server binary exposing tools and
   resources to AI agents.

Components communicate through a **local Unix domain socket API** with an
**auth token**. The daemon is the single source of truth.

The system is organized as a **Rust workspace monorepo** with a SwiftUI app
alongside:

```
apps/macos/RichterApp/         # SwiftUI app
crates/richter-core/           # Domain types, classifier, fingerprint, contracts
crates/richter-daemon/         # Daemon: run mgr, scheduler, API, persistence, watcher
crates/richter-cli/            # CLI binary
crates/richter-mcp/            # MCP protocol implementation
integrations/                  # Agent-specific integration files
docs/                          # Documentation
adr/                           # Architecture Decision Records
scripts/                       # Build, test, install, demo scripts
```

Key technology choices:

| Choice | Decision | Rationale |
|---|---|---|
| macOS app framework | SwiftUI + AppKit | Native macOS look and feel, Keychain access, SMAppService, UserNotifications |
| Core engine language | Rust | Process supervision, async IO, safety, performance, CLIs |
| Async runtime | Tokio | Industry standard, mature ecosystem, excellent macOS support |
| Inter-component communication | Unix domain socket + HTTP (Axum) | Fast, local-only, no port conflicts, standard HTTP tooling |
| Auth | Random 256-bit token, 0600 file | Simple, effective for local IPC, no PKI overhead |
| Persistence | SQLite (WAL mode) via sqlx | Zero-config, single-file, excellent for local workloads |
| Serialization | serde (JSON) | Standard, human-readable for debugging, good enough perf |
| CLI framework | clap 4 + derive | Mature, typed, good error messages |
| File watching | notify crate (FSEvents backend) | Cross-platform API, macOS-native events underneath |
| MCP implementation | rmcp crate | MCP spec compliant, stdio + HTTP/SSE transports |
| Build system | Cargo workspace + Xcode project | Standard for Rust, standard for Swift |
| Logging | tracing + tracing-subscriber (JSONL) | Structured, performant, good filtering |

## Rationale

### Why SwiftUI App + Rust Daemon (not all-Rust or all-Swift)

An all-Swift daemon would lack Rust's process supervision safety, async
performance, and ecosystem for CLI tools. An all-Rust app would require
significant FFI for macOS-native features (Keychain, SMAppService,
UserNotifications, menu bar) and would produce a non-native feel.

Splitting responsibilities — SwiftUI for presentation and macOS integration,
Rust for orchestration and CLI — gives each component the best tooling for
its job.

### Why Unix Domain Socket (not TCP or DBus)

- Unix sockets are local-only by default (no firewall concerns).
- No port conflicts (the socket is a filesystem path).
- 0600 permissions provide filesystem-level access control.
- HTTP over Unix socket (via Axum) is well-supported and debuggable.
- TCP would require binding to localhost and managing port discovery.
- DBus is not a macOS-native IPC mechanism.

### Why Monorepo

A single repository ensures:

- Atomic changes across the daemon, CLI, MCP server, and SwiftUI app.
- Shared dependency versions (Cargo workspace).
- Consistent CI across all components.
- Easier contributor onboarding (one `git clone`).

The tradeoff is a larger repository, but with ~4 Rust crates and one Swift
package, the scale is manageable.

### Why SQLite (not PostgreSQL or filesystem-only)

- Zero configuration: no database server to manage.
- Single-file: easy backup, restore, reset.
- WAL mode: concurrent reads during writes.
- Bundled via `rusqlite` or `sqlx`: no system dependency.
- More than sufficient for expected workload (thousands of events/day, not
  millions/second).

A filesystem-only approach (JSONL, flat files) would require building index
and query infrastructure that SQLite gives for free.

### Why Tokio (not async-std or smol)

Tokio is the most widely adopted Rust async runtime with the largest
ecosystem of compatible libraries (axum, sqlx, hyper, tower). It has
excellent macOS support and proven production use.

## Alternatives Considered

### All-in-one process (Swift app with embedded Rust via FFI)

**Rejected**: Couples the UI lifecycle to the orchestration engine. If the
app crashes or is closed, orchestration stops. The daemon must survive UI
restarts.

### Electron app

**Rejected**: High memory usage, non-native feel, poor macOS integration
(Keychain, UserNotifications), larger binary size. Does not align with
"macOS polish" goal.

### System extension (Endpoint Security)

**Rejected** for default operation. Requires reduced SIP, MDM approval, or
manual user approval in Recovery Mode. Massive friction. The product must
work out of the box without security policy changes. FSEvents + passive
process detection is good enough for the use case. Endpoint Security is
retained as an optional future module behind a feature flag. See ADR 0003.

### gRPC instead of HTTP

**Rejected**: Adds protobuf compilation step, larger dependency footprint,
harder to debug (binary protocol). HTTP/JSON is sufficient for local IPC
with low throughput requirements.

## Consequences

### Positive

- Clean separation of concerns: UI, orchestration, CLI, agent-facing MCP.
- Each component uses the best tooling for its domain.
- Daemon can be restarted, upgraded, and debugged independently of the UI.
- Monorepo enables fast iteration across all components.
- SQLite gives robust persistence with zero operational overhead.

### Negative

- Unix domain socket adds a small IPC latency (~microseconds) between CLI
  and daemon. Negligible for command execution workflows.
- Auth token management adds complexity (generation, rotation, file
  permissions).
- SwiftUI + Rust split means two build systems (Cargo + Xcode) and two
  languages. Mitigated by clear interface boundaries and build scripts.
- Monorepo CI must handle both Rust and Swift building/testing.

### Mitigations

- `scripts/build.sh` and `scripts/test.sh` unify the build/test experience.
- The local API contract is versioned and documented to keep the Swift-Rust
  boundary stable.
- Token management is fully automated; users never interact with it.
