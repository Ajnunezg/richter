# Richter Installation Guide

## Prerequisites

- **macOS 14 (Sonoma) or later.** Apple Silicon is preferred; Intel Macs are
  supported for most features.
- **Xcode Command Line Tools.** Install via `xcode-select --install` or download
  from [developer.apple.com](https://developer.apple.com).
- **Rust toolchain 1.80 or later.** Install via [rustup](https://rustup.rs):
  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Git 2.40+** (bundled with Xcode CLT).

Optional but recommended:

- **[cargo-nextest](https://nexte.st)** for running the test suite faster:
  ```bash
  curl -LsSf https://get.nexte.st/latest/mac | tar zxf - -C ~/.cargo/bin
  ```

## Building from Source

Clone the repository and build all release binaries:

```bash
git clone https://github.com/ajnunezg/richter.git
cd richter
bash scripts/build.sh
```

This produces:

| Binary | Path |
|---|---|
| `richter` (CLI) | `target/release/richter` |
| `richterd` (daemon) | `target/release/richterd` |
| `Richter.app` (macOS app) | `target/release/Richter.app` |
| `richter-mcp` (MCP binary) | `target/release/richter-mcp` |

For a debug build (faster compile, includes symbols):

```bash
bash scripts/build.sh --debug
```

### Build Verification

After building, verify the binaries are functional:

```bash
target/release/richter --version
target/release/richterd --version
```

## Installing the Daemon

Richter uses a user-scoped background service. It never requires `sudo`.

### Option A: Install via the app (recommended)

1. Open `Richter.app`.
2. The app detects the daemon is not running and prompts to install it.
3. Click **Install Daemon**. This registers the daemon as an
   `SMAppService.LoginItem`, so it starts automatically on login.
4. The app connects and the dashboard appears.

### Option B: Install via CLI

```bash
richter install daemon
```

This copies `richterd` to `~/.richter/bin/richterd`, creates a LaunchAgent
plist at `~/Library/LaunchAgents/com.richter.daemon.plist`, and loads it with
`launchctl bootstrap gui/$UID`.

Verify the daemon is running:

```bash
richter doctor
```

The output should show:
```
✔ daemon running (pid 12345)
✔ local API reachable
✔ SQLite database initialized
```

## Shell Integration Setup

Shell integration adds `richter` awareness to your shell prompt and enables
automatic shim PATH injection.

### Zsh

```bash
richter install shell
```

This appends to `~/.zshrc`:
```bash
# Richter shell integration
export RICHTER_HOME="$HOME/.richter"
export PATH="$RICHTER_HOME/shims:$PATH"
eval "$(richter shell init zsh)"
```

### Bash

```bash
richter install shell --bash
```

### Fish

```bash
richter install shell --fish
```

Restart your shell or `source` your rc file for changes to take effect.

## Shim Installation

Shims are thin wrappers that intercept build/test/lint commands and route them
through Richter. They live in `~/.richter/shims/` and must appear before system
package-manager paths in `PATH`.

Install the default shim set:

```bash
richter install shims
```

This creates symlinks or small wrapper scripts for all supported tools:
`npm`, `pnpm`, `yarn`, `bun`, `node`, `npx`, `cargo`, `go`, `python`, `pytest`,
`uv`, `ruff`, `make`, `cmake`, `ninja`, `xcodebuild`, `swift`, `gradle`, `mvn`,
`bazel`, `turbo`, `nx`, `deno`, `tsc`, `eslint`, `jest`, `vitest`, `playwright`.

Each shim rewrites the invocation to `richter run --shim-name <tool> -- <args>`.

To install only specific shims:

```bash
richter install shims --tools cargo,npm,pnpm,go
```

To list installed shims:

```bash
richter shims list
```

Verify shims are working:

```bash
which cargo    # Should show ~/.richter/shims/cargo
cargo --help    # Should still work normally
```

## MCP Configuration for Agents

Richter provides an MCP (Model Context Protocol) server that AI coding agents can
use to query status, join runs, and claim paths.

### Claude Code

Generate and install the MCP config snippet:

```bash
richter install mcp --agent claude
```

This creates or updates `~/.claude/claude_desktop_config.json` (or the
project-local `.mcp.json`) with a Richter MCP server entry.

### Codex (Codex CLI)

```bash
richter install mcp --agent codex
```

This adds the Richter MCP server to the Codex MCP configuration.

### Other Agents (Generic stdio MCP)

```bash
richter install mcp --agent generic --output ~/.config/mcp/richter.json
```

For any agent that supports stdio MCP, configure it to run:

```bash
richter-mcp stdio
```

### Verifying MCP

```bash
richter doctor --mcp
```

Should show:
```
✔ MCP server binary present
✔ Claude Code MCP configured
✔ Codex MCP configured
```

## Verifying Installation

Run the full doctor check:

```bash
richter doctor
```

Expected output:

```
Richter Doctor
═══════════════
✔ daemon        running (pid 12345, uptime 2m)
✔ api           reachable at ~/.richter/daemon.sock
✔ database      initialized (schema v1, 0 events)
✔ shell         integrated (zsh)
✔ shims         27 installed, PATH correct
✔ hooks         claude configured, codex configured
✔ mcp           server binary ready
✔ permissions   no issues
✔ model         none configured (optional)
✔ fs watcher    3 repos watched
```

## Uninstallation

### Remove daemon and service

```bash
richter uninstall daemon
```

This unloads the LaunchAgent, removes the plist, and deletes `~/.richter/bin/richterd`.

### Remove shell integration

```bash
richter uninstall shell
```

This removes the Richter lines from your shell rc files.

### Remove shims

```bash
richter uninstall shims
```

This deletes the `~/.richter/shims/` directory and cleans up PATH references.

### Remove MCP configs

```bash
richter uninstall mcp --agent claude
richter uninstall mcp --agent codex
```

### Remove everything

```bash
richter uninstall --all
```

Or manually:

```bash
rm -rf ~/.richter
rm -f ~/Library/LaunchAgents/com.richter.daemon.plist
# Remove lines referencing "richter" from your shell rc files
```

### Remove the macOS app

Delete `Richter.app` from `/Applications` or wherever you placed it. The app
does not leave files outside `~/.richter/` and its own bundle.

## Troubleshooting Installation

| Problem | Check |
|---|---|
| Daemon won't start | `richter doctor` — check permissions on `~/.richter/` (should be 0700) |
| Shims not in PATH | `echo $PATH` — ensure `~/.richter/shims` appears before other tool paths |
| "Permission denied" on socket | `ls -la ~/.richter/daemon.sock` — should be `srw-------` |
| "Richter is not installed" | Run `richter install daemon` first |
| App can't connect | Ensure daemon is running: `richter status` |

See `docs/TROUBLESHOOTING.md` for a complete troubleshooting guide.
