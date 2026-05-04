# Richter

**Local agent-control plane for multi-agent AI development on macOS.**

Richter coordinates command execution when running multiple AI coding agents (Codex, Claude Code, Droid, Forge Code, Kimi, MiniMax) simultaneously on the same machine. It prevents duplicate builds/tests, manages resources, and surfaces only what matters.

## Quick Start

```bash
# Clone and build
git clone https://github.com/ajnunezg/richter.git
cd richter
make build

# Install
make install

# Verify
richter doctor

# Run a command through Richter
richter run -- cargo test
```

## Features

- **Run-or-Join**: duplicate commands join the existing run instead of duplicating work
- **Smart Caching**: fresh results are reused when cryptographic fingerprints match
- **Resource Scheduling**: prevents CPU/RAM thrashing from competing agents
- **Important-Output Engine**: surfaces only significant events, not log floods ("holy shit, that's done" quality bar)
- **MCP Server**: exposes Richter to any MCP-compatible AI agent
- **Native macOS App**: menu bar app + full dashboard (SwiftUI, macOS 14+)
- **No Cloud Required**: works fully offline by default

## Architecture

```
┌─────────────────────────────────────────────┐
│         Richter macOS App (SwiftUI)          │
│       menu bar + dashboard window            │
└──────────────────┬──────────────────────────┘
                   │ Unix Domain Socket API
┌──────────────────┴──────────────────────────┐
│           Richter Daemon (Rust)              │
│  ┌──────────┐ ┌──────────┐ ┌──────────────┐ │
│  │Run Mgr   │ │Scheduler │ │Importance    │ │
│  │(dedup)   │ │(resource)│ │Engine        │ │
│  └──────────┘ └──────────┘ └──────────────┘ │
│  ┌──────────┐ ┌──────────┐ ┌──────────────┐ │
│  │Supervisor│ │FS Watch  │ │Event Bus     │ │
│  └──────────┘ └──────────┘ └──────────────┘ │
└──────────────────┬──────────────────────────┘
                   │
    ┌──────────────┼──────────────┐
    ▼              ▼              ▼
┌───────┐    ┌────────┐    ┌─────────┐
│  CLI  │    │  MCP   │    │  Shims  │
│richter│    │ Server │    │~/.richter│
└───────┘    └────────┘    └─────────┘
```

## Documentation

| Document | Description |
|----------|-------------|
| [Installation](docs/INSTALL.md) | Build and install guide |
| [User Guide](docs/USER_GUIDE.md) | Using Richter day-to-day |
| [Architecture](docs/ARCHITECTURE.md) | System design and data flow |
| [Integrations](docs/INTEGRATIONS.md) | Agent integrations (Claude, Codex, MCP) |
| [Models](docs/MODELS.md) | Optional LLM pipeline configuration |
| [Security](docs/SECURITY.md) | Security model and best practices |
| [Privacy](docs/PRIVACY.md) | Privacy guarantees |
| [Troubleshooting](docs/TROUBLESHOOTING.md) | Common issues and fixes |

## Architecture Decision Records

- [ADR 0001: Architecture](adr/0001-architecture.md)
- [ADR 0002: Command Fingerprints](adr/0002-command-fingerprints.md)
- [ADR 0003: No Endpoint Security by Default](adr/0003-why-not-global-endpoint-security-by-default.md)
- [ADR 0004: LLM Importance Pipeline](adr/0004-llm-importance-pipeline.md)

## Requirements

- macOS 14.0+ (Apple Silicon or Intel)
- Rust toolchain (1.80+)
- Xcode Command Line Tools

## License

MIT — Copyright (c) 2026 Alberto Nunez
