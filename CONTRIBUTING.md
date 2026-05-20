# Contributing to Richter

Richter is a daemon-driven command de-duplication and caching system for agentic coding assistants.

## Development Setup

```bash
git clone https://github.com/ajnunezg/richter
cd richter
cargo build
cargo test --all
```

## Architecture

- `crates/richter-core` — Shared types, database, fingerprinting, classifier, git utilities
- `crates/richter-daemon` — Background service: API, scheduler, run manager, supervisor, event bus, MCP bridge, mobile gateway
- `crates/richter-cli` — User-facing CLI: run, status, doctor, explain, audit, setup, mobile
- `crates/richter-mcp` — Model Context Protocol server for AI coding agents

## Running the Daemon

```bash
cargo run --bin richter-daemon
# In another terminal:
cargo run --bin richter -- status
cargo run --bin richter -- run -- cargo build
```

## Testing

```bash
# Unit tests
cargo test --all

# Integration tests
cargo test -p richter-daemon --test api_integration
cargo test -p richter-daemon --test run_manager_integration
cargo test -p richter-daemon --test scheduler_integration

# Coverage (requires cargo-llvm-cov)
cargo llvm-cov --all-features --lcov --output-path lcov.info
```

## Test Strategy

1. **Unit tests** cover individual functions and methods across all crates
2. **Integration tests** verify API auth, run-or-join lifecycle, scheduler behavior, and event bus
3. **Concurrency tests** validate concurrent run_or_join calls from multiple simulated agents (8-20 concurrent tasks)

## Code Style

- `cargo fmt` and `cargo clippy` must pass before PR
- Use `tracing` for logging, never `println!` in library code
- Prefer `anyhow::Result` for application errors, `thiserror` for library errors

## CI

CI runs on every push and PR:
- `cargo build --all-targets`
- `cargo test --all`
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo audit` (security advisories)
- `cargo deny check` (license compliance)
