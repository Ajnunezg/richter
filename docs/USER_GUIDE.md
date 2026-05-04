# Richter User Guide

## Getting Started

Richter is a local control plane that coordinates AI coding agents running on your
Mac. It prevents duplicate builds and tests, manages CPU and memory pressure, and
shows only the events that matter — without getting in the way.

After installing (see `docs/INSTALL.md`), Richter works automatically. You don't
change how you use your AI agents. When an agent runs `cargo test`, `pnpm test`,
or any supported build/lint command, Richter intercepts it, checks if an equivalent
command is already running, and either joins the existing run, returns a cached
result, or executes it.

### Quick Start

```bash
# 1. Verify installation
richter doctor

# 2. Open the dashboard
open /Applications/Richter.app

# 3. Run a command through Richter to see it working
richter run -- echo "Hello from Richter"

# 4. Start your AI agents as usual — Richter handles the rest
```

### Menu Bar Icon

The Richter menu bar icon gives at-a-glance status:

| Icon State | Meaning |
|---|---|
| Calm (gray) | Idle. No active runs. |
| Active (blue) | Runs in progress. |
| Warning (yellow) | Queued runs, resource pressure, or conflicts. |
| Critical (red) | Failed commands or errors requiring attention. |

Click the menu bar icon to see:
- Active runs with subscriber counts
- Queued runs waiting for resources
- Most recent important event
- System pressure (CPU/memory load)
- Quick-open dashboard
- Pause/resume coordination
- Temporarily disable shims

## Understanding the Dashboard

### Now Page

The "Now" page is your default view. It shows only what needs attention right now:

- **Important Events** — the highest-importance events across all repos (test
  failures, build errors, conflicts, resource warnings).
- **Active Heavy Runs** — which repos are running builds or large test suites
  right now, and how many agents are sharing each run.
- **Duplicate Work Saved** — a counter and breakdown of how many command
  executions were avoided through run-joining and caching. This is the core
  value metric.
- **System Pressure** — current CPU load, memory pressure, and active process
  count.

The "Now" page is designed to be quiet. If nothing is wrong, it shows almost
nothing. That's by design.

### Repos Page

Lists all detected Git repositories and their worktrees.

For each repo you see:
- Repository name and root path
- Branch and dirty state (clean ✗, modified ●, staged ○)
- HEAD SHA (abbreviated)
- Number of active agents working in this repo
- Active and queued runs
- Per-worktree breakdown when multiple worktrees exist

Click a repo to drill into its runs, agents, and history.

### Runs Page

The runs timeline shows every command execution (and deduplication) that Richter
has processed.

Columns:
- **Time** — when the command was submitted
- **Command** — the original command line (truncated)
- **Class** — build, test, lint, typecheck, install, dev-server, etc.
- **Status** — active, completed, failed, cancelled, joined, cached
- **Subscribers** — how many agents are sharing this run
- **Duration** — wall time
- **Repo** — which repository

Filters:
- By repo, agent, command class, status, time range
- Quick filter: "My runs", "Failures only", "Active only"

Click a run to see:
- Full command and fingerprint hash
- All subscribers and when they joined/detached
- Exit code
- Output summary (first failure, error counts)
- Link to full compressed log
- Cache status (if result is cached, shows original run ID)

### Agents Page

Shows all AI coding agents Richter has detected.

For each agent:
- Agent name and type (Claude Code, Codex, Droid, etc.)
- Current working directory and Git repo/branch
- Active command (if any)
- Claimed path leases
- Last seen timestamp

Agents are auto-detected by observing shell activity flowing through shims and
by passive process tree monitoring. You don't need to register them manually.

### Events Page

The complete event log, searchable and filterable.

- **Important events** are shown first (sorted by importance score).
- **All events** are available behind a toggle.
- Full-text search across event titles, summaries, and raw log content.
- Export filtered events as JSONL.

### Settings Page

Configure Richter behavior:

- **Integrations** — shell, shims, hooks, MCP install/uninstall
- **Models** — configure optional LLM providers for summarization
- **Privacy** — redaction rules, retention periods, log cleanup
- **Resources** — CPU/memory concurrency limits
- **Notifications** — which events trigger macOS notifications

### Doctor Page

Runtime health checks:

- Daemon status and uptime
- Local API reachability
- Database schema version
- Installed shims count and PATH position
- Hook configuration status for each agent type
- MCP server status
- File watcher repos
- Model provider connectivity (if configured)
- Filesystem permissions

Run the same checks from the command line with `richter doctor`.

## Running Commands Through Richter

### Via Shims (Automatic)

Once shims are installed and in your PATH, commands flow through Richter
automatically:

```bash
# These all go through Richter automatically
cargo test
pnpm test
npm run build
go test ./...
pytest
```

You don't need to do anything differently. The shim intercepts the command,
sends it to the daemon, and the daemon decides: run, join an existing run, or
return a cached result.

### Via Explicit CLI

If you prefer not to use shims, or need a one-off:

```bash
richter run -- cargo test --lib
richter run -- pnpm test -- --grep "auth"
richter run --reuse -- cargo build --release
```

Flags:
- `--reuse` — prefer an existing run or cache even if fingerprints don't match
  (use with caution; may cause false joins)
- `--no-cache` — force a fresh execution, bypass cache
- `--isolated` — run in an isolated process group (no joining by other agents)
- `--tag <name>` — label the run for later reference

### What Happens When You Run a Command

```
$ cargo test --lib
   Richter: fingerprint a8f3b2c... | class=test
   Richter: joined existing run #42 (2 subscribers, started 41s ago)
   ...
   test result: ok. 42 passed; 0 failed; 0 ignored
   Richter: run #42 completed (exit 0, cached for 10m)
```

The agent sees standard output. Richter adds a one-line status message. That's it.

## Understanding Run-or-Join

Run-or-join is the core mechanism that prevents duplicate work.

When two agents run the same command (same tool, same args, same repo state,
same lockfiles), Richter detects the match via fingerprint comparison and makes
the second agent a **subscriber** to the first agent's run.

```
Agent A: cargo test --lib
           │
           ▼
Agent B: cargo test --lib
           │
           ▼
   Richter: matches fingerprint → joins existing run
           │
           ▼
   Both agents receive the same output and exit code.
   One process ran. Two agents got results.
```

### When Richter Joins

Richter joins commands when the **fingerprint** matches. The fingerprint includes:

- Canonicalized command and all arguments
- Command class (build, test, lint, etc.)
- Repository identity and worktree path
- HEAD commit SHA
- Dirty tree hash (uncommitted changes)
- Staged and unstaged diff hashes
- Relevant lockfiles (Cargo.lock, pnpm-lock.yaml, etc.)
- Toolchain versions (rustc, node, go, python)
- Relevant environment variables (RUSTFLAGS, NODE_ENV, etc.)
- Working directory
- Test target/subset (if inferrable)

If any of these differ, Richter considers it a different command and may run it
separately.

### When Richter Doesn't Join

- Destructive commands (unless explicitly allowlisted)
- Interactive commands (Richter detects TTY needs)
- Unknown commands (unless configured)
- Commands with resource locks held by another process on the same path
- Subset/superset relationships that can't be proven deterministically

## Understanding Caching

When a command completes successfully and is cacheable (determined by its class
and policy), Richter stores the exit code, output summary, and a link to the full
log. If the same fingerprint appears again before the TTL expires, the cached
result is returned instantly.

Cache TTLs by command class (defaults, configurable):

| Class | Default TTL |
|---|---|
| `test` | 10 minutes |
| `lint` | 15 minutes |
| `typecheck` | 15 minutes |
| `build` | 5 minutes |
| `install` | disabled by default |
| `format` | 30 minutes |
| `dev-server` | not cached |
| `destructive` | never cached |
| `unknown` | never cached |

Cache TTLs are configurable per-repo in `.richter/config.toml`:

```toml
[commands.test]
cache_ttl = "20m"
```

### Cache Invalidation

Richter tracks the inputs that go into each fingerprint. When any input changes
— a new HEAD SHA, modified lockfile, changed env var — the fingerprint changes
and the cache entry for the old fingerprint is stale. Richter does not attempt
to invalidate cache entries retroactively; it simply doesn't serve them for new
fingerprints.

## Understanding Queuing

When resources are constrained (a heavy build is already running in the same
repo, or global heavy-run concurrency is at its limit), Richter queues new
requests instead of executing them immediately.

```
Agent C: cargo test --all
           │
           ▼
   Richter: heavy build #88 already running in this repo
   Richter: queued for ~2m (1 ahead of you)
   ...
   Richter: heavy build #88 completed
   Richter: starting your test now
```

Queue behavior:
- FIFO within the same resource class and repo
- Queue position is reported to the requesting agent
- If the queue TTL expires, the request is rejected with a clear message
- Agents can cancel their queued request

## Configuring Policies

### Global Config

`~/.richter/config.toml`:

```toml
[global]
watched_dirs = ["/Users/you/projects"]

[resources]
max_heavy_runs_per_repo = 1
max_heavy_runs_global = 3
max_light_runs_per_repo = 3
max_light_runs_global = 8
max_install_runs_per_repo = 1
max_dev_servers_per_repo = 2

[retention]
runs_days = 7
events_days = 30

[notifications]
notify_on = ["test_failure", "build_error", "resource_pressure"]
coalesce_window = "5m"
max_notifications_per_hour = 10
```

### Per-Repo Config

`.richter/config.toml` (in the repo root):

```toml
[repo]
name = "my-project"

[commands.build]
cache = true
ttl = "5m"
dedupe = true

[commands.test]
cache = true
ttl = "10m"
dedupe = true

[[commands.rules]]
match = "pnpm install"
class = "install"
cache = false
dedupe = false
resource_lock = "node_modules"

[[commands.rules]]
match = "pnpm run dev"
class = "dev-server"
dedupe = false
allow_multiple = false

[resources]
max_heavy_runs = 1
```

Per-repo config overrides global config for that repo.

## Managing Worktrees

Richter detects Git worktrees automatically. If you use worktrees for parallel
agent work, Richter tracks which agent is in which worktree.

### Listing Worktrees

```bash
richter worktree list
```

### Creating a Managed Worktree

```bash
richter worktree create --agent claude --from main
```

This creates a new worktree for the specified agent detached from `main`.
Richter recommends worktrees when multiple agents are detected in the same
dirty worktree, since simultaneous edits in the same working directory can
cause conflicts.

Richter **never** automatically moves an agent's working directory. It only
recommends worktree creation.

### Deduplication Across Worktrees

If two worktrees share the same HEAD SHA, lockfiles, and relevant config,
Richter may deduplicate commands across them. This is conservative: the
fingerprint must match exactly. Path-dependent build artifacts (like
`target/` directories) are typically worktree-specific, so Richter treats
them as separate unless the user explicitly configures shared build
directories.

## Path Leases

Path leases let agents declare intent to work on specific files, preventing
accidental conflicts.

### Claiming a Path

```bash
richter claim src/auth.rs --ttl 30m --agent "Claude Code"
```

Or via MCP: `richter_claim_paths`

### Releasing a Path

```bash
richter release src/auth.rs
```

Or via MCP: `richter_release_paths`

### Lease Visibility

The dashboard shows claimed paths per agent. If two agents claim the same path
or if an agent modifies a path claimed by another agent, Richter surfaces one
important event.

Leases are **advisory** — they don't enforce filesystem locks. They help
coordinate agents but don't block direct filesystem access.

## Model Provider Configuration (Optional)

Richter can use language models for better event summarization and ambiguous
decision support, but this is entirely optional. See `docs/MODELS.md` for
detailed configuration.

## Troubleshooting Common Issues

### Commands aren't going through Richter

Check PATH ordering:
```bash
echo $PATH | tr ':' '\n' | grep -n richter
```

The `~/.richter/shims` entry should appear early. If the system tool path
appears first, your shell rc file may need adjustment.

Verify shims:
```bash
which cargo
# Should show: /Users/you/.richter/shims/cargo
```

### "Daemon not running" error

```bash
richter doctor           # Diagnose
richter install daemon   # Reinstall if needed
```

### Too many "joined existing run" messages

These are normal and desirable. The alternative is duplicate work burning CPU.
If you want to see detail about what was joined, use `richter events`.

### Cached result seems stale

The cache TTL may be too long for your workflow. Adjust in `.richter/config.toml`:
```toml
[commands.test]
cache_ttl = "2m"
```

Or bypass cache for a single run:
```bash
richter run --no-cache -- cargo test
```

### Agent not detected

Richter detects agents by observing process trees and shim invocations. If an
agent is not detected:
- Ensure the agent runs commands through the shell (shims must be in PATH).
- Check that the agent's parent process is visible to the current user.
- Run `richter agents` to see what Richter does see.

## Best Practices for Multi-Agent Development

1. **Use different worktrees for different agents.** This prevents file-level
   editing conflicts while allowing Richter to deduplicate tests that are
   identical across worktrees.

2. **Tag your runs.** Use `--tag` to label runs for later analysis:
   ```bash
   richter run --tag "refactor-auth" -- cargo test -p auth
   ```

3. **Check the dashboard before starting a heavy run.** The "Now" page shows
   if a similar run is already in progress.

4. **Configure repos with heavy test suites.** Add per-repo config to
   `.richter/config.toml` with appropriate cache TTLs and concurrency limits.

5. **Use path leases for shared files.** If two agents might edit the same
   configuration or schema file, have each claim it first.

6. **Review the "Duplicate Work Saved" stat.** It should grow over time.
   If it's low, your agents may be running different commands (different test
   filters, different build profiles) that can't be deduplicated. Consider
   standardizing test commands across agents.

7. **Don't micromanage the queue.** Richter's scheduler handles priority and
   backpressure. Let it do its job — it's designed to be quiet unless
   something is wrong.
