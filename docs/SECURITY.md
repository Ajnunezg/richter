# Richter Security Documentation

## Security Model Overview

Richter is a local agent-control plane. Its security model is built on four
principles:

1. **No root.** All components run as the current user. No system extensions,
   no privileged helpers, no `sudo`, no `setuid`.
2. **No cloud by default.** No data leaves the machine unless the user
   explicitly configures an optional model provider.
3. **Redaction first.** Secrets are stripped from stored output and from any
   text sent to model providers.
4. **Model output is advisory.** No LLM output can directly authorize
   destructive commands or modify runtime policy.

Richter's scope is coordination, not enforcement. It does not aim to
sandbox agents or prevent malicious code execution — that's the job of the
operating system and the user's own security practices.

## No-Root Principle

Richter operates entirely within the user's session:

| Component | User | Privileges |
|---|---|---|
| `Richter.app` (SwiftUI) | Current user | Normal app sandbox (if signed) |
| `richterd` (daemon) | Current user | Normal user LaunchAgent |
| `richter` (CLI) | Current user | Inherits calling shell's privileges |
| `richter-mcp` (MCP) | Current user | Inherits calling process's privileges |
| Shell shims | Current user | Inherits calling shell's privileges |

The daemon registers as a user LaunchAgent (SMAppService.LoginItem). It never
requests or requires root access. If a permissions issue arises (such as
watching a directory the user doesn't own), Richter surfaces an error rather
than requesting privilege escalation.

### Why No Root

- Root access violates the principle of least privilege.
- System extensions (Endpoint Security, Kernel Extensions) require reduced
  SIP, MDM approval, or manual approval in Recovery Mode — unacceptable
  friction for a developer tool.
- Without root, Richter cannot silently observe all processes — but it doesn't
  need to. FSEvents on watched directories plus passive process polling is
  sufficient for command coordination.
- No root means no risk of a Richter bug escalating to system compromise.

## No-Cloud-by-Default Principle

Richter makes zero outbound network calls by default. The only outbound
connections possible are:

1. **Optional model provider API calls** (OpenAI, Anthropic, DeepSeek, etc.)
   — explicitly configured by the user.
2. **Optional MCP HTTP transport** bound to localhost — never exposed to the
   network.
3. **Git operations** — Richter runs `git rev-parse`, `git worktree list`,
   etc., which may fetch from configured remotes if the user's Git is
   configured to auto-fetch. These are the user's own Git operations, not
   Richter-initiated network calls.

Richter never phones home. There is no telemetry, no analytics, no update
checker, no crash reporter that sends data off-device.

## Secrets Redaction

Richter redacts secrets at two points:

1. **Capture time** — when command output is captured, the redaction engine
   processes it before it is written to the compressed log file or stored
   in the database.
2. **Model call time** — before text is sent to a model provider, the
   redaction engine processes it again (belt-and-suspenders).

### What Gets Redacted

| Pattern | Example | Replacement |
|---|---|---|
| OpenAI API keys | `sk-proj-abc123...` | `[REDACTED:openai_key]` |
| Anthropic API keys | `sk-ant-api03-...` | `[REDACTED:anthropic_key]` |
| DeepSeek API keys | `sk-abc123...` | `[REDACTED:deepseek_key]` |
| GitHub tokens | `ghp_abc123...` | `[REDACTED:github_token]` |
| GitHub PATs | `github_pat_abc...` | `[REDACTED:github_pat]` |
| Generic bearer tokens | `Bearer eyJhbG...` | `[REDACTED:bearer_token]` |
| AWS access keys | `AKIAIOSFODNN7...` | `[REDACTED:aws_key]` |
| AWS secret keys | 40-char base64 near an access key | `[REDACTED:aws_secret]` |
| GCP service account keys | JSON with `"private_key"` | `[REDACTED:gcp_key]` |
| Azure connection strings | `DefaultEndpointsProtocol=...` | `[REDACTED:azure_connstr]` |
| Database URLs | `postgres://user:pass@host/db` | `[REDACTED:db_url]` |
| Private keys | `-----BEGIN PRIVATE KEY-----` blocks | `[REDACTED:private_key]` |
| ENV values matching key patterns | `SECRET=abc`, `TOKEN=xyz` | `[REDACTED:env_value]` |
| Long hex/base64 strings in env context | Typically tokens/secrets | `[REDACTED:suspected_token]` |

### What Does NOT Get Redacted

- File paths (within workspace)
- Command names and arguments
- Test names and results
- Error messages (unless they contain a secret pattern)
- Source code snippets in test output
- Build output
- Linter output

### Redaction Limitations

Richter's redaction is **best-effort, pattern-based**. It will catch most
common secret formats but cannot guarantee catching novel or obfuscated
secrets. Users should:

- Never hardcode secrets in source code (use environment variables or a
  secrets manager).
- Use `.env` files that are not checked in (and not printed by commands).
- Avoid `echo $SECRET` in terminal sessions observed by Richter.

## Keychain Storage

> **⚠️ Not yet implemented.** Keychain storage for provider API keys is planned
> but not yet implemented. Currently, API keys entered in the Swift app's
> Settings view are held in the `@State` property `modelAPIKey` on
> `RichterSettings` — they live only in the Swift app's process memory and are
> **never persisted to disk**. Closing the app clears the key. The daemon
> receives the key over the Unix socket when needed and holds it in memory only
> for the duration of the API call.

All provider API keys (OpenAI, Anthropic, DeepSeek, etc.) are intended to be
stored in the macOS Keychain:

```
Service: com.richter.openai-api-key
Account: default
Access Group: $(TeamIdentifier).com.richter

Service: com.richter.anthropic-api-key
Account: default
Access Group: $(TeamIdentifier).com.richter
```

Once implemented, the Richter SwiftUI app will manage Keychain entries via the
Security framework:

```swift
SecItemAdd(...)
SecItemCopyMatching(...)
SecItemUpdate(...)
SecItemDelete(...)
```

API keys are **never** written to:

- Config files (`~/.richter/config.toml` or `.richter/config.toml`)
- The SQLite database
- Environment variables (after initial setup)
- Log files

When the daemon needs to make a model call, it requests the key from the app
via the local API. The key is held in daemon memory only for the duration of
the API call and then dropped.

### Keychain Access

When implemented, the Keychain item will be created with an access control
setting that requires the Richter app's bundle ID for access. If the daemon is
running as a separate process, it receives the key over the authenticated Unix
socket, not via direct Keychain access.

## Unix Domain Socket Auth

The local API uses a Unix domain socket at `~/.richter/daemon.sock`:

```
Permissions: 0600 (owner read/write only)
Owner: current user
Group: current user's primary group
```

An auth token is required on every request:

- Generated at daemon startup (256 bits of random from `/dev/urandom`, hex-encoded).
- Written to `~/.richter/daemon.token` with `0600` permissions.
- Sent as `Authorization: Bearer <token>` in the HTTP header (or first frame
  if using the binary protocol).
- Rotated on every daemon restart.
- Never logged; never persisted in the database.

The token file's permissions prevent other users on the system from reading it.
Since the socket is a Unix domain socket, it's not accessible from other
machines (no network exposure).

## Workspace Boundary Enforcement

Richter's file watcher and lease system enforce workspace boundaries:

1. **FSEvents watches only configured directories.** The daemon watches
   directories listed in `~/.richter/config.toml` or `.richter/config.toml`
   (per-repo). It never watches system directories or arbitrary paths.

2. **Path lease validation.** When an agent requests a path lease via MCP
   or CLI, Richter validates that the path is within a watched workspace.
   Paths outside watched directories are rejected.

3. **Symlink resolution.** Before granting a lease or following a file
   change, Richter resolves symlinks and rejects paths that escape the
   workspace root after resolution.

4. **Path traversal rejection.** Requests containing `..` segments that
   would escape the workspace root are rejected.

5. **No global filesystem scanning.** Richter never recursively scans the
   entire disk. It only examines directories explicitly configured as
   watched.

## No Model Output Directly Authorizing Destructive Commands

The importance pipeline uses LLMs for **summarization and classification
only**. Model output can never:

- Directly authorize a command execution
- Modify a run-or-join decision
- Change a policy gate
- Execute shell commands
- Modify the filesystem
- Interact with agent processes

The output schema for model calls is strictly constrained to importance
classification and summarization fields. Extraneous fields are dropped.
Commands are only executed through the deterministic path (classifier →
fingerprint → policy check → run-or-join decision).

### Why This Matters

An LLM reading build output or test logs is reading untrusted text. Test
names, error messages, or log lines could contain prompt injection attacks.
By restricting model output to classification/summarization only and never
using it to authorize actions, Richter is immune to prompt injection
escalating to command execution.

## LLM Payload Preview and Audit

Two mechanisms provide transparency into what Richter sends to model
providers:

### Payload Preview

Available in Settings → Models → "Preview last payload." Shows:

- The redacted text sent to the model.
- The model's response.
- Token counts and latency.

This is useful for verifying that redaction is working correctly and that
no secrets are leaking.

### Audit Log

All model calls are recorded in the `model_calls` SQLite table. Access via:

```bash
richter models calls --last 50
```

The audit trail includes provider, model, input hash (SHA-256 of the
redacted input), token counts, latency, and cost estimate. The actual
input/output text is stored as content-addressed compressed files, linked
by hash from the audit table.

## Threat Model

### What Richter Protects Against

| Threat | Protection |
|---|---|
| Duplicate work wasting CPU/memory | Run-or-join deduplication |
| Resource exhaustion from concurrent heavy builds | Resource scheduler with concurrency limits |
| Accidental secret leakage to logs | Redaction engine at capture time |
| Accidental secret leakage to model providers | Redaction engine at model call time |
| Agent conflicts on shared files | Advisory path leases |
| Flood of unimportant events | Importance pipeline + notification policy |
| Unauthorized access to the local API | Unix socket permissions + auth token |
| API key theft from config files | Keys held in app memory only (Keychain storage planned but not yet implemented) |

### What Richter Does NOT Protect Against

| Threat | Why Not |
|---|---|
| Malicious AI agents | Richter coordinates, doesn't sandbox. Use OS-level controls. |
| Malicious shell commands | Agents can still run arbitrary commands. Richter only intercepts known build/test/lint commands. |
| Prompt injection in model calls | Model output is advisory only. Cannot authorize actions. |
| Filesystem attacks escaping workspace | OS-level sandboxing, not Richter's role. |
| Network-based attacks | Richter doesn't control network access. |
| Privilege escalation via Richter bugs | No root access means a Richter bug can't escalate beyond user privileges. |
| Side-channel attacks | Richter doesn't aim to prevent timing or power analysis. |
| Physical access attacks | FileVault. Richter data lives in the user's home directory. |

### Attack Surface Analysis

| Component | Attack Surface | Mitigation |
|---|---|---|
| Unix domain socket | Local processes could attempt to connect | 0600 permissions + 256-bit auth token |
| MCP server (stdio) | Agent process could send malicious JSON-RPC | Input validation, bounded tool output |
| MCP server (HTTP/SSE) | Only bound to Unix socket, not network | Same auth as local API |
| Shell shims | Shims are user-writable. A compromised process could modify them | Shims are user-owned files; OS file permissions apply |
| Model API calls | Intercepted in transit (TLS) | HTTPS with certificate validation |
| SQLite database | File is user-readable | 0600 permissions on database directory |
| Config files | User-editable | 0600 permissions on `~/.richter/` |

## Reporting Security Issues

If you discover a security issue in Richter:

1. **Do not file a public issue.** Instead, email `dewclaw@hey.com` with
   details. Please include:
   - A description of the issue.
   - Steps to reproduce.
   - Affected versions (or commit SHA).
   - Whether the issue is exploitable by a local process, by an AI agent,
     by a model provider, or requires physical access.

2. **Expected response time**: 72 hours for acknowledgment, 14 days for
   initial assessment.

3. **Disclosure**: We follow coordinated disclosure. Please allow 90 days
   for a fix before public disclosure.

4. **Scope**: Security issues in Richter itself. Issues in dependencies
   (tokio, axum, sqlx, etc.) should be reported to those projects.
   We will update dependencies promptly when fixes are available.

### Responsible Disclosure Hall of Fame

Security researchers who report valid issues will be acknowledged here
(with permission) after the fix is released.
