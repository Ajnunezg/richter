# ADR 0003: Why Not Global Endpoint Security by Default

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter needs to observe when AI coding agents execute build, test, and lint
commands. The most comprehensive approach would use macOS Endpoint Security
(ES) framework, which can intercept all process executions system-wide. This
would allow Richter to see every command every agent runs, regardless of how
they invoke it.

However, ES has significant deployment friction. We need to decide: should
ES be the default observation mechanism, or should we use a lighter-weight
approach?

## Decision

**Richter does not use Endpoint Security by default.** The default observation
mechanism is:

1. **Shell shims** — thin wrappers in `PATH` that intercept known build/test
   tools and route them through `richter run`.

2. **Passive process detection** — periodic sampling of the process tree
   (via `libproc` / `sysctl`) to detect known agent CLIs and their child
   processes.

3. **FSEvents on configured workspace directories** — detect file changes
   that indicate command execution (build artifacts, test results, log files).

4. **MCP agent connections** — agents that connect to Richter's MCP server
   are tracked explicitly.

Endpoint Security is retained as an **optional future module** behind a
disabled feature flag (`endpoint-security`). It will not block the v1.0
release.

## Rationale

### Deployment Friction of Endpoint Security

Endpoint Security requires one of the following for third-party applications:

1. **System Extension** (preferred by Apple): Requires the application to be
   distributed with a provisioning profile, requires user approval in System
   Settings → Privacy & Security, and may require MDM for enterprise
   deployment. The approval dialog is intimidating for typical users.

2. **Full Disk Access + SIP reduction**: Historically possible but Apple has
   been closing these paths. Not reliable for a product targeting broad
   distribution.

3. **MDM-only deployment**: Some ES capabilities require MDM profiles.
   Unacceptable for an individually-installed developer tool.

In practice, getting a user to approve an Endpoint Security system extension
means:
- They must navigate to System Settings → Privacy & Security.
- They must find the correct section (which changes between macOS versions).
- They must click "Allow" on a warning that says the extension can "monitor
  all system activity."
- On Apple Silicon, they may need to change startup security policy in
  Recovery Mode.

This is **unacceptable friction** for a developer tool that should "just work"
after `richter install daemon`.

### What We Lose Without ES

Without ES, Richter cannot:

- Observe commands executed by agents that bypass shell shims (e.g., agents
  that call `execve` directly without going through a shell).
- Observe commands in containers or VMs.
- Observe commands executed by agents running as other users.
- Guarantee that every command in every process tree is intercepted.

### What We Keep Without ES

With shims + passive detection + MCP:

- All commands executed through standard shell workflows are intercepted
  (this covers the vast majority of AI agent usage).
- Agents that use the MCP server gain deeper coordination capabilities.
- File system changes are detected, providing a secondary signal.
- The user experience remains frictionless — no scary approval dialogs.

### Why This Is Good Enough

1. **Most AI agents use shell commands.** Codex, Claude Code, Droid, Forge
   Code, Kimi, MiniMax — they all execute `npm test`, `cargo build`, etc.
   through a shell. Shell shims catch these.

2. **Shims are transparent.** Users and agents don't need to change behavior.
   The shim intercepts at the `PATH` level, so `npm test` works identically
   whether or not Richter is installed.

3. **MCP provides an upgrade path.** Agents that support MCP can get deeper
   integration (status queries, run-or-join, path leases) without requiring
   ES-level system access.

4. **FSEvents provides a safety net.** Even if an agent bypasses shims,
   file changes in watched directories are detected. This can't prevent
   duplicate execution, but it surfaces the event.

## Alternatives Considered

### Require Endpoint Security as a hard dependency

**Rejected** for v1.0. Would make installation too difficult for the target
audience. Could be reconsidered for v2.0 if user demand justifies it and
Apple simplifies the approval flow.

### Use `execsnoop` / DTrace

**Rejected**: DTrace requires SIP to be partially disabled on modern macOS.
Same friction as ES but with less capability.

### Use `auditd` (OpenBSM)

**Rejected**: Deprecated by Apple, not guaranteed to work on future macOS
versions.

### Use `proc_pidinfo` / Kdebug

**Rejected**: `Kdebug` is a private API and requires SIP disabled. Not
viable for a distributed application.

### Hybrid: ES for those who enable it, shims for everyone else

**Accepted as the long-term strategy.** The architecture supports an
`endpoint-security` feature flag. When enabled (by a user who is willing
to approve the system extension), Richter can supplement shim-based
interception with ES-level observation. The shim path remains the default
and the failsafe.

## Future Path

The `endpoint-security` feature flag will be scaffolded in the codebase:

```toml
# Cargo.toml (richter-daemon)
[features]
default = []
endpoint-security = ["dep:endpoint-security-rs"]
```

When enabled:

1. The daemon requests ES entitlement.
2. A System Extension is bundled with the app.
3. The user approves the extension in System Settings.
4. ES events for `ES_EVENT_TYPE_EXEC` are received.
5. Observed commands go through the same classifier → fingerprint →
   run-or-join pipeline as shim-intercepted commands.
6. ES observation supplements (does not replace) shims and MCP.

The ES module is documented as experimental and opt-in. The product must
not depend on it.

## Consequences

### Positive

- Zero-friction installation: no scary system dialogs, no SIP changes.
- Works on all macOS 14+ systems out of the box.
- Shims provide sufficient coverage for the primary use case.
- MCP provides an upgrade path for capable agents.
- ES remains available as an optional enhancement for power users.

### Negative

- Cannot guarantee 100% command interception. Agents that use direct
  `execve` calls for tools that are not shimmed will be invisible.
- Passive process detection has a polling delay (default 30s), so a
  newly spawned agent may not be detected immediately.
- If an agent modifies PATH to remove shims before executing commands,
  those commands bypass Richter. (An agent actively trying to bypass
  Richter is an adversarial scenario outside our threat model.)

### Mitigations

- `richter doctor` verifies that shims are in PATH and working.
- The dashboard warns if shims appear to be missing or bypassed.
- MCP is the recommended integration for agents that support it.
- Per-repo hook installation ensures Claude Code and Codex are configured
  out of the box.
