# Richter Security Policy

## Summary

Richter is a local agent-control plane for macOS. It operates entirely within
the user's session with **no root privileges** and **no cloud dependencies by
default**. Secrets are redacted before storage and before any optional model
provider calls. Model output is advisory only and cannot authorize
destructive commands.

## Key Principles

- **No root.** All components run as the current user. No `sudo`, no system
  extensions, no privileged helpers.
- **No cloud by default.** No data leaves the machine unless the user
  explicitly configures an optional model provider.
- **Redaction first.** API keys, tokens, private keys, and credentials are
  stripped from captured output before storage and before model calls.
- **Model output is advisory.** LLMs summarize and classify; they never
  authorize commands, modify policy, or execute actions.

## Full Documentation

See [`docs/SECURITY.md`](docs/SECURITY.md) for:

- Complete security model
- Secrets redaction specification (what gets redacted, how)
- Keychain storage for provider API keys
- Unix domain socket authentication (0600 permissions, 256-bit auth token)
- Workspace boundary enforcement
- Threat model (what Richter protects against, what it doesn't)
- Attack surface analysis

See [`docs/PRIVACY.md`](docs/PRIVACY.md) for:

- No telemetry guarantee
- What data stays local
- What data goes to model providers (only if configured)
- Log retention and cleanup
- Data storage locations
- User controls

## Reporting Security Issues

**Do not file a public issue.** Email `dewclaw@hey.com` with:

- Description of the issue
- Steps to reproduce
- Affected version or commit SHA
- Whether the issue is exploitable locally, by an AI agent, by a model
  provider, or requires physical access

Expected response: 72 hours acknowledgment, 14 days initial assessment.

We follow coordinated disclosure. Please allow 90 days for a fix before
public disclosure.

## Scope

Security issues in Richter itself. Issues in dependencies should be reported
to those projects; we will update promptly when fixes are available.

## Supported Versions

| Version | Supported |
|---|---|
| Latest release | ✅ |
| `main` branch | ✅ (development) |
| Older releases | ❌ |
