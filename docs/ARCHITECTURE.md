# Richter Architecture

## System Overview

Richter is a local agent-control plane for macOS. It sits between AI coding agents and the
shell, intercepting build/test/lint commands, deduplicating redundant work across multiple
concurrent agents, managing compute resources, and surfacing only the events that matter.
Richter operates entirely on the local machine; it sends no data off-device unless the user
explicitly configures optional LLM-based summarization.

The system consists of four main components and three integration surfaces:

```
┌─────────────────────────────────────────────────────────┐
│                     macOS User Session                    │
│                                                           │
│  ┌──────────────┐  ┌──────────┐  ┌───────────────────┐  │
│  │ Richter App   │  │  richter  │  │  AI Coding Agents  │  │
│  │  (SwiftUI)    │  │   (CLI)   │  │  (Codex, Claude,   │  │
│  │               │  │           │  │   Droid, etc.)     │  │
│  └──────┬───────┘  └─────┬─────┘  └────────┬──────────┘  │
│         │                │                  │              │
│         │   Unix domain  │                  │              │
│         │   socket API   │                  │              │
│         └────────┬───────┘                  │              │
│                  │               ┌──────────┴──────────┐  │
│                  │               │  Shell Shims         │  │
│                  │               │  (~/.richter/shims)  │  │
│                  │               └──────────┬──────────┘  │
│                  │                          │              │
│         ┌────────┴──────────────────────────┴──────────┐  │
│         │            Richter Daemon (Rust)              │  │
│         │                                               │  │
│         │  ┌─────────────┐  ┌──────────────────────┐   │  │
│         │  │  Core Engine │  │  MCP Server          │   │  │
│         │  │  - classifier│  │  (stdio + HTTP)      │   │  │
│         │  │  - fingerprint│ │                       │   │  │
│         │  │  - scheduler │  │                       │   │  │
│         │  │  - run mgr   │  │                       │   │  │
│         │  └──────┬──────┘  └──────────────────────┘   │  │
│         │         │                                     │  │
│         │  ┌──────┴──────────────────────────────────┐ │  │
│         │  │  Persistence (SQLite, WAL mode)          │ │  │
│         │  │  - repos, runs, events, cache, leases    │ │  │
│         │  └─────────────────────────────────────────┘ │  │
│         │                                               │  │
│         │  ┌──────────────────────────────────────────┐│  │
│         │  │  File Watcher (FSEvents)                  ││  │
│         │  │  - workspace dirs, Git state, lockfiles   ││  │
│         │  └──────────────────────────────────────────┘│  │
│         └──────────────────────────────────────────────┘  │
│                                                           │
│  ┌─────────────────────────────────────────────────────┐ │
│  │  macOS Keychain (provider API keys)                  │ │
│  └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

- **Richter App** — SwiftUI macOS menu-bar app plus full-window dashboard. The primary
  user-facing surface. Communicates with the daemon exclusively via the local Unix domain
  socket API. Displays active runs, queued runs, important events, repo/worktree state,
  agent status, and system pressure. Stores provider API keys in the macOS Keychain.
  Never calls external model APIs directly — all model calls are mediated by the daemon.

- **Richter Daemon** (`richterd`) — A user-scoped background service (SMAppService
  LoginItem) written in Rust. Owns all orchestration logic: command classification,
  fingerprinting, run-or-join deduplication, resource scheduling, process supervision,
  event emission, persistence, the local API server, and the MCP server. Runs as the
  current user, never as root. Survives UI app restarts.

- **Richter CLI** (`richter`) — A terminal binary for direct interaction. Commands
  include `doctor`, `status`, `repos`, `agents`, `runs`, `events`, `run`, `install`,
  `uninstall`, and `simulate`. Communicates with the daemon via the same Unix domain
  socket API. If the daemon is not running, it prompts to start it.

- **Richter MCP Server** — An MCP-compliant server exposing tools and resources
  so AI coding agents can query status, join runs, claim paths, and retrieve
  summaries without shell interception. Supports both stdio transport (for
  process-spawned agents) and local HTTP/SSE transport.

- **Shell Shims** — A directory of thin wrappers (`~/.richter/shims/`) placed ahead
  of system package-manager/build tools in `PATH`. Each shim rewrites the invocation
  to `richter run --shim-name <tool> -- <original args>`, so commands automatically
  flow through the Richter daemon for classification and deduplication.

- **Agent Hooks** — Claude Code and Codex hook configuration files that enable
  PreToolUse/PostToolUse notifications and agent-local MCP configuration so agents
  can query Richter state without shell interception.

## Crate Map

```
crates/
├── richter-core        # Data types, contracts, classifier, fingerprint engine,
│                       #   importance pipeline, config model, redaction engine,
│                       #   persistence traits, event model
├── richter-daemon      # Process supervisor, scheduler, run manager, local API server,
│                       #   file watcher (FSEvents), MCP server, persistence
│                       #   implementation (SQLite), model provider clients
├── richter-cli         # CLI binary (clap), all `richter` subcommands,
│                       #   daemon API client, shim installer, hook generator
└── richter-mcp         # MCP protocol implementation, tool/resource registry,
                        #   stdio + HTTP transports, agent-facing schemas
```

| Crate | Purpose | Key Dependencies |
|---|---|---|
| `richter-core` | Shared domain types, classifier, fingerprint, redaction | `serde`, `blake3`, `sha2`, `regex`, `chrono`, `uuid` |
| `richter-daemon` | Runtime orchestration, API, persistence, MCP host | `richter-core`, `tokio`, `axum`, `sqlx`, `notify`, `rmcp` |
| `richter-cli` | Terminal user interface, shim/hook installers | `richter-core`, `clap`, `tokio` |
| `richter-mcp` | MCP transport and schema layer | `richter-core`, `rmcp`, `serde_json` |

Dependencies are unidirectional: `richter-daemon` and `richter-cli` depend on
`richter-core`. `richter-mcp` depends on `richter-core`. `richter-core` depends on
no other workspace crate. The SwiftUI app depends only on the daemon's local API and
the FFI contract layer (to be implemented in `richter-core`).

## Data Flow

### Command Execution Lifecycle

```
Agent issues command
        │
        ▼
Shell shim rewrites to: richter run -- <command>
        │
        ▼
richter-cli sends RunRequest to daemon via Unix socket
        │
        ▼
┌─────────────────────────────────────────────────┐
│ Daemon: run_or_join(request)                     │
│                                                  │
│  1. Classify command (deterministic parser)      │
│     → build | test | lint | typecheck | ...      │
│                                                  │
│  2. Compute fingerprint                          │
│     → argv hash + repo + HEAD + dirty-tree       │
│       + lockfiles + toolchain + env              │
│                                                  │
│  3. Check active runs for matching fingerprint   │
│     ├── Match found → JOIN as subscriber         │
│     │   - Stream existing output tail            │
│     │   - Return same exit code on completion    │
│     │   - Emit "joined existing run" event       │
│     │                                             │
│     ├── Cache hit → RETURN cached result         │
│     │   - Return exit code + summary             │
│     │   - Emit "cache hit" event                 │
│     │                                             │
│     └── No match → EXECUTE new run               │
│         - Acquire resource locks                 │
│         - Spawn supervised child process         │
│         - Capture stdout/stderr                  │
│         - Parse output for test/build errors      │
│         - Store logs + summary                   │
│         - Emit "completed" event                 │
│                                                  │
│  4. Importance pipeline (optional)               │
│     - Deterministic: parse JUnit/TAP/etc.        │
│     - Cheap model: classify/summarize            │
│     - Frontier model: ambiguous adjudication     │
│                                                  │
│  5. Update cache, release locks, notify UI       │
└─────────────────────────────────────────────────┘
```

### Event Flow

```
Command Event
     │
     ▼
┌────────────────┐
│ Event Emitter   │────► SQLite events table (all events)
└───────┬────────┘
        │
        ▼
┌────────────────────┐
│ Importance Pipeline │
│ 1. Deterministic    │──► Pass-through (always)
│ 2. Cheap model      │──► Optional, configurable
│ 3. Frontier model   │──► Optional, budget-limited
└───────┬────────────┘
        │
        ▼
┌────────────────────┐
│ Notification Policy │
│ - Coalesce          │──► macOS UserNotification (only high/critical)
│ - Rate-limit        │──► Dashboard "Now" page (important only)
│ - Filter            │──► Events page (all, searchable)
└────────────────────┘
```

## Local API Design

The daemon exposes a local API over a Unix domain socket with an auth token.

```
Socket: ~/.richter/daemon.sock (0600 permissions)
Token:  ~/.richter/daemon.token  (0600 permissions, 256-bit random hex)
```

All requests include the token in an `Authorization: Bearer <token>` header (HTTP)
or as the first frame (custom binary protocol).

### HTTP API Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/health` | Daemon health, uptime, version |
| `GET` | `/status` | Global status summary |
| `GET` | `/repos` | List known repositories |
| `GET` | `/repos/:id` | Single repo detail + worktrees |
| `GET` | `/agents` | List detected agents |
| `GET` | `/runs` | List runs (paginated, filterable) |
| `GET` | `/runs/:id` | Single run detail + subscribers |
| `GET` | `/runs/:id/output` | Stream/retrieve run output |
| `GET` | `/events` | Paginated event stream |
| `GET` | `/events/important` | Important events only |
| `POST` | `/runs` | Submit a command to run-or-join |
| `POST` | `/leases` | Claim a path/file lease |
| `DELETE` | `/leases/:id` | Release a lease |
| `GET` | `/settings` | Current config (redacted) |
| `PATCH` | `/settings` | Update live settings |
| `GET` | `/doctor` | Installation health checks |
| `POST` | `/install/shell` | Install shell integration |
| `POST` | `/install/shims` | Install PATH shims |
| `POST` | `/install/mcp` | Generate MCP config |
| `POST` | `/install/hooks` | Generate agent hook config |
| `POST` | `/simulate` | Run simulation scenario |

### Private `/api/mcp` Sub-path

The MCP server for HTTP/SSE transport binds at the same socket under `/api/mcp`.
Stdio MCP uses a separate process spawned by the daemon that communicates over an
internal channel.

## Persistence Layer

Richter uses **SQLite** with **WAL mode** for all durable state.

### Why SQLite

- Zero configuration. No separate database server to manage.
- Single-file database. Easy to back up, restore, or delete.
- WAL mode allows concurrent reads while writes are in progress.
- Excellent performance for the expected workload (thousands of events per day,
  not millions per second).
- Bundled via `rusqlite` or `sqlx`, so no system dependency.

### Schema (Logical)

```
repositories
  id, root_path, git_common_dir, name, created_at

worktrees
  id, repo_id, path, branch, head_sha, upstream, dirty_state,
  lockfiles_hash, created_at, updated_at

agents
  id, name, type, cwd, worktree_id, pid, last_seen_at

command_invocations
  id, agent_id, repo_id, worktree_id, command_class, argv,
  fingerprint, cwd, env_hash, submitted_at

runs
  id, invocation_id, status (pending|active|completed|failed|cancelled),
  pid, exit_code, started_at, completed_at, fingerprint, cache_key,
  resource_class, subscriber_count

run_subscribers
  run_id, invocation_id, agent_id, attached_at, detached_at,
  received_exit_code

run_cache
  fingerprint_hash, exit_code, output_path, summary_json,
  created_at, expires_at, ttl_seconds

events
  id, run_id, agent_id, repo_id, type, severity, title,
  summary_json, created_at

important_events
  event_id, importance_score, category, model_call_id,
  should_notify_user, should_surface_to_agents

decisions
  id, invocation_id, decision_type (run|join|cache|queue|reject),
  reason, fingerprint, model_call_id, created_at

leases
  id, agent_id, path, lease_type (advisory|exclusive),
  expires_at, created_at

model_calls
  id, provider, model, input_hash, output_json, latency_ms,
  tokens_in, tokens_out, cost_estimate, created_at

settings
  key, value, updated_at

plugin_manifests
  id, name, version, manifest_json, enabled, installed_at
```

### Migrations

Migrations are applied sequentially at daemon startup. Each migration is a numbered
SQL file in `crates/richter-daemon/migrations/`. The `schema_version` pragma or a
dedicated `_migrations` table tracks applied migrations. Migrations are
forward-only; rollback is handled by the user deleting the database and restarting.

### Retention

- Raw run output files older than configurable `retention.runs_days` are deleted.
- Events older than `retention.events_days` are pruned.
- Cache entries expire per their individual TTLs.
- Default retention: 7 days for runs, 30 days for events.

## Security Model

Richter's security posture is based on four principles:

1. **No root.** All components run as the current user. No system extensions,
   no privileged helpers, no `sudo`. The daemon registers as a user LaunchAgent
   (SMAppService LoginItem).

2. **No cloud by default.** No data leaves the machine unless the user explicitly
   configures an optional model provider. Even then, only redacted, bounded
   payloads are sent.

3. **Redaction first.** Secrets are stripped from all stored output and from any
   text sent to model providers. The redaction engine runs at capture time and
   again before model calls.

4. **Model output is advisory.** No LLM output can directly authorize destructive
   commands, modify the run-or-join decision, or change policy. The deterministic
   engine is authoritative; model output is a summary/classification hint at most.

### Auth Token

The Unix domain socket auth token is generated at daemon startup, written to
`~/.richter/daemon.token` with `0600` permissions, and required on every API call.
The token is rotated on daemon restart. The SwiftUI app and CLI read this file
to authenticate.

### API Key Storage

Provider API keys (OpenAI, Anthropic, DeepSeek, etc.) are stored in the macOS
Keychain, not in config files or the database. The `richter-app` SwiftUI process
manages keychain entries; the daemon receives keys via the local API when it needs
to make model calls, and holds them in memory only for the duration of the call.

### Workspace Boundary Enforcement

- FSEvents watches only explicitly configured workspace directories.
- File path leases are advisory and scoped to watched directories.
- The daemon will not follow symlinks that escape the workspace root.
- Path traversal (`..`) in lease requests is rejected.

## Resource Scheduling Architecture

```
                    ┌──────────────┐
                    │ Run Requests  │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │ Classifier +  │
                    │ Fingerprint   │
                    └──────┬───────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
         ┌────▼───┐  ┌────▼───┐  ┌─────▼─────┐
         │ JOIN   │  │ CACHE  │  │  EXECUTE   │
         │ active │  │ hit    │  │  new run   │
         └────────┘  └────────┘  └─────┬─────┘
                                       │
                              ┌────────▼────────┐
                              │ Resource Check   │
                              │ - CPU pressure   │
                              │ - Memory pressure│
                              │ - Repo concurrency│
                              │ - Global concurrency│
                              └────────┬────────┘
                                       │
                           ┌───────────┼───────────┐
                           │           │           │
                      ┌────▼───┐ ┌─────▼────┐ ┌───▼────┐
                      │  RUN   │ │  QUEUE   │ │ REJECT │
                      │  now   │ │  wait    │ │  busy  │
                      └────────┘ └──────────┘ └────────┘
```

### Resource Classes

| Class | Examples | Repo Limit | Global Limit |
|---|---|---|---|
| `heavy-build` | `cargo build`, `xcodebuild` | 1 | 2 |
| `heavy-test` | `cargo test`, `pytest --all` | 1 | 3 |
| `light-lint` | `cargo clippy`, `eslint` | 3 | 8 |
| `install` | `pnpm install`, `bundle install` | 1 | 2 |
| `dev-server` | `npm run dev`, `cargo watch` | 2 | 6 |

Limits are configurable per-repo and globally in `~/.richter/config.toml`.

### Backpressure

When a resource class is at capacity, new run requests are queued (FIFO within
priority bands). The requesting agent receives a "queued" response. If the queue
depth exceeds a threshold or the wait exceeds a TTL, the request is rejected and
the agent receives a clear error message.

### Ctrl-C and Cancellation

- If one subscriber sends SIGINT (Ctrl-C), only that subscriber is detached.
  The underlying run continues if other subscribers remain.
- If the leader (first subscriber) cancels and no other subscribers remain,
  the process group is killed.
- Agents can explicitly cancel their subscription via MCP (`richter_run_cancel`).

## Importance Pipeline

The importance pipeline determines which events are surfaced prominently and which
remain in the raw event log.

### Tier 1: Deterministic (always active)

Parse structured output from common tools:

- **JUnit XML** — extract test count, failures, errors, skipped, first failure message
- **TAP** — parse plan and failure lines
- **Cargo test output** — extract test result summary, failure locations
- **pytest output** — parse failure reports, tracebacks
- **ESLint** — count errors/warnings, group by rule
- **TypeScript (`tsc`)** — extract error count and first error
- **Go test** — parse `--- FAIL` lines and summary
- **xcodebuild** — parse test summary, build errors
- **Bazel** — parse test summary, build failure targets

Results are scored deterministically: `test_failure = 80`, `build_error = 75`,
`lint_error = 40`, `test_pass = 5`, etc.

### Tier 2: Cheap Model (optional, configurable)

For output that doesn't have a parser, or for summarizing parsed results, a small
fast model can classify and summarize.

Default configurable provider: **DeepSeek V4 Flash** (fast, cheap) or any local model
via **Ollama/MLX/llama.cpp**.

Input: redacted, truncated first 4KB of output + deterministic parse results.
Output: strict JSON with `importance`, `category`, `title`, `summary`,
`should_notify_user`, `should_surface_to_agents`, `recommended_action`, `confidence`.

### Tier 3: Frontier Model (optional, budget-limited)

For ambiguous, high-impact decisions:

- Whether one test run covers another (subset/superset test coverage)
- Repeated failures across multiple agents
- Complex conflict summaries involving multiple repos
- High-cost queue decisions

Default configurable provider: **GPT-5.5** or **Claude Opus 4.7**.

Budget-limited: maximum N calls per day, maximum cost per month. Only the most
ambiguous cases (confidence < 0.7 from cheap model) are escalated.

## Component Interaction Diagram

```
┌────────────┐                  ┌───────────────────────────────────┐
│  SwiftUI   │──Unix Socket────▶│          Richter Daemon            │
│  App       │                  │                                    │
└────────────┘                  │  ┌──────────┐   ┌──────────────┐  │
                                │  │  Axum    │   │  MCP Server   │  │
┌────────────┐                  │  │  HTTP    │◀──│  (rmcp)       │  │
│  richter   │──Unix Socket────▶│  │  Server  │   │               │  │
│  CLI       │                  │  └────┬─────┘   └──────┬────────┘  │
└────────────┘                  │       │                │           │
                                │  ┌────▼────────────────▼──────┐   │
┌────────────┐                  │  │     Run Manager             │   │
│  AI Agent  │──shim/richter───▶│  │  - classify                 │   │
│  (shell)   │                  │  │  - fingerprint              │   │
└────────────┘                  │  │  - run-or-join              │   │
                                │  │  - cache                    │   │
┌────────────┐                  │  └────┬────────────────────────┘   │
│  AI Agent  │──MCP (stdio)────▶│       │                            │
│  (MCP)     │                  │  ┌────▼────────────────────────┐   │
└────────────┘                  │  │     Resource Scheduler       │   │
                                │  │  - CPU/memory monitor        │   │
                                │  │  - Concurrency limits        │   │
                                │  │  - Queue manager             │   │
                                │  └────┬────────────────────────┘   │
                                │       │                            │
                                │  ┌────▼────────────────────────┐   │
                                │  │     Process Supervisor        │   │
                                │  │  - spawn child processes      │   │
                                │  │  - capture stdout/stderr      │   │
                                │  │  - detect orphans             │   │
                                │  │  - handle Ctrl-C              │   │
                                │  └────┬────────────────────────┘   │
                                │       │                            │
                                │  ┌────▼────────────────────────┐   │
                                │  │     Event & Importance        │   │
                                │  │  - deterministic parsers      │   │
                                │  │  - redaction engine           │   │
                                │  │  - cheap model caller         │   │
                                │  │  - frontier model caller      │   │
                                │  │  - notification policy        │   │
                                │  └────┬────────────────────────┘   │
                                │       │                            │
                                │  ┌────▼────────────────────────┐   │
                                │  │     Persistence               │   │
                                │  │  - sqlx/rusqlite (SQLite)     │   │
                                │  │  - WAL mode                  │   │
                                │  │  - migrations                │   │
                                │  │  - compressed log storage    │   │
                                │  └──────────────────────────────┘   │
                                │                                    │
                                │  ┌──────────────────────────────┐   │
                                │  │     File Watcher              │   │
                                │  │  - notify (FSEvents)          │   │
                                │  │  - Git state polling          │   │
                                │  │  - lockfile detection         │   │
                                │  └──────────────────────────────┘   │
                                └───────────────────────────────────┘
```

## Startup Sequence

```
1. SwiftUI app launches
2. App checks if daemon is running (socket check)
   ├── Running → connect, show dashboard
   └── Not running
       3. App spawns richterd as LoginItem via SMAppService
       4. Daemon:
          a. Apply SQLite migrations
          b. Generate auth token, write to daemon.token
          c. Bind Unix socket, start Axum server
          d. Start FSEvents watcher on configured directories
          e. Discover Git repos and worktrees
          f. Start resource monitor (CPU, memory)
          g. Run passive agent detection
          h. Log "ready"
       5. App connects, verifies token, shows dashboard
```
