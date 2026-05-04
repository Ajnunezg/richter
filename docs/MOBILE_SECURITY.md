# Richter Mobile Security

## Security Model Overview

Richter Mobile inherits Richter's local-first, zero-trust-on-the-network
security philosophy. Every component of the mobile security model is
designed around these principles:

1. **The network is hostile.** All communication is encrypted and
   authenticated. No plaintext ever traverses the network.
2. **The daemon is the trust root.** The Mac daemon (`richterd`) is the
   authoritative source for authorization. The mobile device proves its
   identity to the daemon, not the other way around.
3. **No passwords, no accounts.** Passwords are phishable, reusable, and
   breachable. Richter uses cryptographic keypairs established via a
   physical pairing ceremony.
4. **Least privilege.** Every device has a scoped capability set. A device
   that only needs to view runs cannot approve dangerous actions.
5. **Defense in depth.** Multiple layers — TLS, certificate pinning,
   request signing, biometric gating, timestamp replay protection — ensure
   that a failure in one layer does not compromise the system.

For the formal trust model and pairing design, see
**[ADR 0006: Mobile Pairing and Trust Model](../adr/0006-mobile-pairing-and-trust-model.md)**.

## Pairing Ceremony

The pairing ceremony is the foundation of all mobile security. It establishes
the cryptographic trust relationship between the mobile device and the Mac
daemon.

### Why QR Codes

QR-based pairing was chosen over PIN codes, passwords, OAuth, and Bluetooth
pairing because:

| Method | Entropy | Requires Physical Presence | Passive Eavesdropper |
|---|---|---|---|
| QR code (256-bit token) | 256 bits | ✅ Camera must see screen | ❌ Camera required |
| PIN (6 digits) | ~20 bits | ❌ Can be shoulder-surfed | ✅ Easy to observe |
| Password | Variable (often low) | ❌ Can be keylogged | ❌ Can be phished |
| OAuth / account | Depends on provider | ❌ Remote possible | ❌ Phishing vector |
| Bluetooth pairing | ~20-40 bits | ✅ Proximity | ⚠️ Relay attacks |

The QR code encodes a 32-byte (256-bit) random pairing token, the daemon's
network address, port, TLS certificate SHA-256 fingerprint, and Ed25519
public key. The token is single-use with a 120-second expiry window.

### Ceremony Steps

1. **Initiation** — The user explicitly clicks "Pair Mobile Device" in the
   Mac menu bar (Settings → Mobile Devices → Pair New Device). This is the
   only way to start pairing; there is no programmatic API and no headless
   mode.

2. **Token generation** — The daemon generates:
   - `pairing_token`: 32 random bytes from the system CSPRNG
     (`getrandom(2)` on macOS).
   - `pairing_session_id`: A UUID v4.
   - Expiry timestamp: `now + 120 seconds`.

3. **QR display** — The daemon constructs a JSON payload with all the
   information the mobile app needs and encodes it as a QR code. The QR
   code is displayed on the Mac's primary screen only — it is never sent
   over the network or saved to disk.

4. **QR scan** — The mobile app scans the QR code using the device camera.
   The payload is parsed and validated: version field, expiry timestamp,
   valid port range.

5. **HTTPS connection** — The mobile app connects to the daemon's address
   and port over HTTPS. The app verifies the daemon's TLS certificate
   against the SHA-256 fingerprint from the QR payload. If the fingerprints
   do not match, the connection is aborted — this is a potential MITM attack.

6. **Pairing request** — The mobile app sends `POST /mobile/pair` with:
   ```json
   {
     "pairing_token": "<base64url-token>",
     "device_pubkey": "<base64-encoded-ed25519-pubkey>",
     "device_name": "Alberto's iPhone",
     "device_model": "iPhone 16 Pro",
     "device_os": "iOS 18.2"
   }
   ```

7. **Token validation** — The daemon looks up the pairing session by token.
   It verifies: the session exists, the token has not expired, and the
   session has not been used previously. If any check fails, the daemon
   returns 401 and the session is invalidated.

8. **Device registration** — The daemon:
   - Assigns a unique `device_id` (UUID v7, time-ordered).
   - Stores the device's Ed25519 public key.
   - Records device metadata (name, model, OS).
   - Assigns the default capability set.
   - Marks the pairing session as used.
   - Logs a pairing audit event.

9. **Confirmation** — The daemon returns the `device_id` and assigned
   capabilities to the mobile app.

10. **Secure storage** — The mobile app stores the device keypair, daemon
    address, port, certificate fingerprint, and daemon public key in the
    platform's secure storage (iOS Keychain, Android Keystore +
    EncryptedSharedPreferences).

### What the QR Code Contains

```json
{
  "v": 1,
  "daemon_addr": "192.168.1.42",
  "daemon_hostname": "albertos-macbook-pro.local",
  "daemon_port": 19557,
  "cert_fingerprint_sha256": "a1b2c3d4e5f67890abcdef1234567890abcdef1234567890abcdef1234567890",
  "daemon_pubkey": "MCowBQYDK2VwAyEA...",
  "pairing_token": "dGhpcyBpcyBhIHRlc3QgdG9rZW4...",
  "expires_at": "2026-05-04T14:02:00Z"
}
```

## Device Authentication

After pairing, every API request (except `/mobile/pair`) is authenticated
using Ed25519 request signing.

### Request Signing Protocol

Every request includes three headers:

```
X-Richter-Device-Id: dev_01JQ2XYZ...
X-Richter-Signature: dGhpcyBpcyBhIHNpZ25hdHVyZQ...
X-Richter-Timestamp: 1714867320
```

The signature is computed over:

```
<HTTP_METHOD>\n
<REQUEST_PATH>\n
<X-Richter-Timestamp>\n
<SHA256(request_body)>
```

For example, a GET request to `/mobile/now` at timestamp 1714867320 with
no body would sign:

```
GET\n
/mobile/now\n
1714867320\n
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

The last line is the SHA-256 of an empty string
(`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).

A POST request to `/mobile/approve` with body
`{"action_id":"act_123"}` would sign:

```
POST\n
/mobile/approve\n
1714867320\n
<sha256 of {"action_id":"act_123"}>
```

### Signature Verification on the Daemon

1. Extract `X-Richter-Device-Id`, `X-Richter-Signature`, and
   `X-Richter-Timestamp`.
2. Verify the timestamp is within ±30 seconds of daemon time. Reject if
   outside this window (clock skew tolerance; also prevents indefinite
   replay).
3. Look up the device's Ed25519 public key by `X-Richter-Device-Id`.
4. Reconstruct the signing payload.
5. Verify the Ed25519 signature. Reject on failure with 401 (no additional
   information to avoid oracle attacks).
6. Check the `(device_id, timestamp, body_hash)` tuple against a bloom
   filter of recently seen requests. Reject if present (replay detected).
7. Add the tuple to the bloom filter.

### Replay Protection

The combination of timestamp window (±30s) and bloom filter provides
replay protection:

- **Timestamp window**: An attacker cannot replay a request after 30
  seconds because the daemon rejects stale timestamps.
- **Bloom filter**: An attacker cannot replay a request within the 30-second
  window because the daemon tracks seen `(device_id, timestamp, body_hash)`
  tuples.
- **Clock sync**: On first connection, the daemon sends its current time
  in the response headers (`X-Richter-Server-Time`). The mobile app can
  detect clock drift and alert the user if the device clock is off by
  more than 10 seconds.

### Why Ed25519

Ed25519 was chosen for device keypairs because:

- **Small keys**: 32-byte private keys, 32-byte public keys. Compact
  enough for QR codes and efficient on mobile devices.
- **Fast signing**: ~50,000 operations per second on modern phone CPUs.
  Request signing is imperceptible (<1ms).
- **Deterministic signatures**: No entropy source needed per signature
  (unlike ECDSA), eliminating a class of side-channel and RNG-failure
  attacks.
- **Widely implemented**: Available in libsodium, ring, and modern crypto
  libraries. Expo's `expo-crypto` can bridge to native Ed25519
  implementations.
- **No ASN.1 or certificate complexity**: Raw public keys, no X.509
  parsing needed.

## Scope Enforcement

Every device has a capability set enforced by the Mobile Gateway:

| Capability | Default | What It Allows |
|---|---|---|
| `read:status` | ✅ | View agent status, Now view |
| `read:runs` | ✅ | Browse run history |
| `read:events` | ✅ | View event stream |
| `read:repos` | ✅ | View repo list and health |
| `read:search` | ✅ | Search events and runs |
| `write:approve` | ✅ | Approve pending agent actions |
| `write:deny` | ✅ | Deny pending agent actions |
| `write:cancel` | ✅ | Cancel running agents |
| `write:revoke` | ✅ | Self-revoke this device |

Capabilities are checked on every request at the Mobile Gateway layer,
before the request reaches the daemon core. A request that requires a
capability the device does not have returns 403:

```json
{
  "error": "forbidden",
  "message": "Device does not have capability: write:approve",
  "required_capability": "write:approve",
  "device_capabilities": ["read:status", "read:runs", "read:events"]
}
```

The Mac user can modify a device's capabilities at any time from
**Settings → Mobile Devices → [Device] → Capabilities**. Changes take
effect immediately — active WebSocket connections from the device are
terminated and must be re-established with the new capability set.

### Capability Design Principles

- **Default permissive for reads**: A paired device can see everything by
  default. This is the "companion" model — viewing is the primary use case.
- **Default permissive for writes**: Approve, deny, cancel, and self-revoke
  are enabled by default. The user explicitly pairs a device; they trust it.
- **Opt-in restriction**: The user can restrict capabilities if they want
  a read-only device (e.g., a tablet left in a shared space).
- **No admin from mobile**: The `admin:pair` capability (authorize new
  device pairing) is Mac-only. It cannot be granted to mobile devices.

## TLS Requirements

### Daemon TLS Identity

- The daemon generates a self-signed X.509 certificate at first launch
  (if no user-provided certificate exists).
- The certificate uses a 2048-bit RSA key or Ed25519 key, SHA-256
  signature algorithm.
- The certificate includes the daemon's hostname and local IP addresses
  as Subject Alternative Names.
- Users can provide their own certificate (e.g., from a local CA, or a
  LetsEncrypt certificate if the daemon is exposed via a domain).
- TLS 1.3 is preferred; TLS 1.2 is accepted as a fallback.

### Certificate Pinning

The mobile app pins the daemon's TLS certificate fingerprint:

- **First pairing**: The certificate SHA-256 fingerprint is embedded in
  the QR code. The mobile app stores it in the Keychain/Keystore.
- **Every connection**: The mobile app verifies the presented certificate's
  SHA-256 fingerprint matches the stored fingerprint. Uses the platform's
  TLS library directly (not a JavaScript TLS implementation).
- **Mismatch**: If the fingerprint does not match, the connection is
  aborted with no fallback. The app shows: "Certificate mismatch. The
  daemon's identity has changed. Re-pair your device."
- **No CA trust**: The mobile app does not trust any public CA for daemon
  connections. This eliminates CA compromise as an attack vector.

### Why Not mTLS

We considered mutual TLS (mTLS) where the device also presents a client
certificate. We chose Ed25519 request signing instead because:

- Request signing works identically over HTTP and WebSocket.
- Easier to debug (headers are visible in logs; client certificates are not).
- Avoids platform-specific TLS client certificate quirks on iOS
  (where client certificates in the Keychain have specific requirements).
- Allows the same auth model for WebSocket upgrade requests and regular
  HTTP requests.
- Decouples transport authentication (TLS, certificate pinning) from
  application authentication (Ed25519 signatures).

## Redaction Guarantees

The Mobile Gateway applies redaction before sending data to mobile devices:

- **Command stderr/stdout**: Truncated to last 2,000 characters per stream.
  Full output is available on the Mac app.
- **File paths**: Absolute paths outside workspace roots are redacted to
  `<redacted-path>`. Workspace-relative paths are preserved.
- **Secrets**: The daemon scans event content for common secret patterns
  (API keys, tokens, passwords) and redacts them to `<redacted-secret>`
  before transmission. This is a best-effort scan, not a guarantee.
- **LLM reasoning traces**: Agent reasoning chains (chain-of-thought) are
  truncated to 500 characters for mobile transmission. Full traces are
  available on the Mac.

Redaction is applied at the Mobile Gateway layer, so even if the daemon
core doesn't redact, mobile clients never see full output.

## Approval Safety

Approving or denying an agent action from mobile requires **biometric
confirmation**:

- **iOS**: `LAContext.evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics)`
  — requires Face ID or Touch ID.
- **Android**: `BiometricPrompt` with `BIOMETRIC_STRONG` — requires
  Class 3 biometric (fingerprint, face, or iris with TEE-backed storage).

The biometric gate is enforced **on the device** before the request is
signed and sent:

```
User taps "Approve"
  → System biometric prompt (Face ID / fingerprint)
  → User authenticates
  → App signs the approve request with device private key
  → Request sent to daemon
```

If biometric auth fails or is cancelled, the request is never signed or
sent. The daemon never sees the approval attempt.

### Why Biometric on Approve/Deny

- **Stolen device (unlocked)**: Someone with physical access to your
  unlocked phone cannot approve dangerous agent actions without your
  biometric.
- **Accidental tap**: You can't accidentally approve an action by
  pocket-tapping — the biometric prompt blocks it.
- **Foreground requirement**: The app must be in the foreground to
  trigger biometric auth. A backgrounded app cannot approve actions.

### Biometric Fallback

If biometric auth is unavailable (device doesn't have it, or user hasn't
enrolled), the app falls back to the device passcode:

- **iOS**: `.deviceOwnerAuthentication` (allows passcode fallback).
- **Android**: `DEVICE_CREDENTIAL` authenticator (allows PIN/pattern/password).

This is less secure than biometric-only but still requires physical device
possession and device unlock knowledge.

## Revocation Model

### Self-Revoke (from Mobile)

1. User goes to **Settings → This Device → Revoke Access**.
2. Biometric confirmation required.
3. App signs and sends `DELETE /mobile/revoke`.
4. Daemon immediately marks the device as revoked: deletes its public key
   from the registry, terminates all active connections.
5. Daemon logs a `device.revoked` audit event with device ID and the
   revoker (device self-revoke).

### Mac-Side Revoke

1. User goes to **Settings → Mobile Devices → [Device] → Revoke**.
2. Daemon immediately marks the device as revoked: deletes its public key,
   terminates all active connections.
3. Daemon logs a `device.revoked` audit event with device ID and revoker
   (Mac user).

### What Revocation Means

- **Immediate**: Active connections are terminated within one second
  (daemon closes the WebSocket / refuses new requests).
- **Permanent**: The device cannot re-connect without re-pairing. The old
  device ID and keypair are permanently invalidated.
- **Audited**: Every revocation is logged with timestamp, device ID, and
  revoker identity.
- **No remote wipe**: Revocation does not delete data from the mobile
  device. The app's local cache of historical data persists. The user can
  uninstall the app to clear all local data.

### Revocation Audit Events

```
{
  "event": "device.revoked",
  "device_id": "dev_01JQ2XYZ...",
  "device_name": "Alberto's iPhone",
  "revoked_by": "mac_user",
  "timestamp": "2026-05-04T15:30:00Z"
}
```

## No-Password-Only-Auth Policy

Richter Mobile does not support password-only authentication. This is
a deliberate security policy:

- Passwords can be keylogged.
- Passwords can be phished.
- Passwords can be brute-forced.
- Passwords are often reused across services.
- Passwords stored on a device can be extracted from backups.

The only authentication methods are:

1. **QR-based pairing** (initial trust establishment, requires physical
   presence at the Mac).
2. **Ed25519 request signing** (ongoing authentication, private key in
   secure enclave).
3. **Biometric confirmation** (approval/deny/cancel/revoke actions,
   requires physical device possession + biometric).

If a user cannot use QR pairing (damaged camera), they can use the manual
pairing code entry (the pairing token displayed alongside the QR code).
This still provides 256 bits of entropy and is single-use with 120-second
expiry.

## Audit Logging

The Mobile Gateway logs the following events to the daemon's audit log:

| Event | When | Fields |
|---|---|---|
| `device.paired` | Successful pairing | device_id, device_name, device_model, device_os |
| `device.revoked` | Device revocation | device_id, revoked_by |
| `device.key_rotated` | Device key rotation | device_id |
| `pairing.session_created` | New pairing session | session_id, expires_at |
| `pairing.session_used` | Pairing session consumed | session_id, device_id |
| `pairing.session_expired` | Session expired unused | session_id |
| `pairing.failed` | Failed pairing attempt | reason, client_ip |
| `request.denied` | 401/403 response | device_id, path, reason |
| `action.approved` | Agent action approved from mobile | action_id, device_id |
| `action.denied` | Agent action denied from mobile | action_id, device_id |
| `agent.cancelled` | Agent cancelled from mobile | run_id, device_id |

Audit events are written to the daemon's SQLite database (same WAL-mode
database as run events) and included in the daemon's JSONL event log.
They are viewable from the Mac app's Audit Log view.

## Threat Model

### Threat: Hostile LAN Attacker

**Scenario**: Attacker is on the same local network as the Mac and mobile
device. They can see network traffic, send packets, and spoof DNS/mDNS.

| Attack Vector | Mitigation |
|---|---|
| Port scan finds daemon, tries to connect | Mobile Gateway requires valid Ed25519 signature for all non-pairing endpoints |
| Spoofs Bonjour to redirect mobile app to attacker's server | Certificate pinning: mobile app rejects any certificate that doesn't match the pinned fingerprint |
| Captures HTTPS traffic (MITM with attacker's certificate) | Certificate pinning; no public CA trust |
| Replays a captured signed request | Timestamp check (±30s) + bloom filter prevents replay |
| Tries to pair a malicious device | Pairing requires QR code displayed on Mac screen; must be physically present |
| Captures QR code (photo of screen) | Pairing token is single-use with 120s expiry |

### Threat: Compromised Relay

**Scenario**: User has set up the optional relay. The relay server is
compromised by an attacker.

| Attack Vector | Mitigation |
|---|---|
| Relay reads application data | All traffic is TLS-encrypted (daemon-cert-pinned) + Ed25519-signed. Relay sees only connection metadata |
| Relay modifies requests in transit | Ed25519 signatures cover the entire request; tampered requests fail signature verification |
| Relay injects responses | Mobile app verifies response signatures (if daemon-to-mobile signing is enabled; future enhancement) |
| Relay logs metadata (IPs, timing) | Metadata minimization: relay is designed to log minimal data. User controls relay infrastructure |
| Relay impersonates daemon | Relay does not have the daemon's TLS certificate private key; mobile app pins the daemon's cert |
| Relay impersonates mobile device | Relay does not have the device's Ed25519 private key; cannot sign requests |

### Threat: Stolen Mobile Device

**Scenario**: Attacker steals the user's unlocked phone.

| Attack Vector | Mitigation |
|---|---|
| Attacker opens Richter Mobile, sees agent activity | App shows last-known state; attacker can read cached data (acceptable — it's monitoring data, not secrets) |
| Attacker tries to approve an action | Biometric confirmation required for approve/deny/cancel. Attacker needs Face ID / fingerprint |
| Attacker tries to revoke device | Biometric confirmation required for self-revoke |
| Attacker tries to re-pair to their own daemon | Attacker would need to be at the user's Mac to display a new QR code |
| Attacker extracts data from device backup | Keychain/Keystore items are encrypted and not included in unencrypted backups. Device private key never leaves secure enclave |

### Threat: Compromised Mac

**Scenario**: Attacker gains access to the user's Mac (malware, physical
access).

| Attack Vector | Mitigation |
|---|---|
| Malware reads device public keys from daemon | Public keys only — no secrets. Attacker cannot impersonate a device without the private key |
| Malware adds a malicious device | Pairing requires explicit user action (click "Pair Mobile Device" in menu bar). No programmatic pairing API |
| Malware modifies device capabilities | Requires Mac UI interaction. Audit log records all capability changes |
| Physical access: attacker pairs their own device | Requires unlocking the Mac, navigating to Settings, and clicking "Pair Mobile Device." Attacker with this level of access can do far worse things |

### Threat: Network-Level Denial of Service

**Scenario**: Attacker floods the Mobile Gateway with requests to degrade
service.

| Attack Vector | Mitigation |
|---|---|
| SYN flood on daemon port | Daemon port bound to localhost or LAN interface only (not exposed to internet by default). OS-level SYN cookies |
| HTTP request flood | Rate limiting: 60 requests/minute per device, 300 requests/minute per IP. Excess returns 429 |
| Slowloris (slow HTTP headers) | Request timeout: 10 seconds for headers, 30 seconds for body. Connections exceeding timeout are terminated |
| WebSocket flood (many connections) | Maximum 3 concurrent WebSocket connections per device. Maximum 20 total WebSocket connections |

## Security Configuration Reference

### Daemon Configuration (richterd.toml)

```toml
[mobile]
enabled = true
port = 19557
bind_address = "0.0.0.0"  # LAN only; change to 127.0.0.1 for localhost-only
tls_cert = "/Users/alberto/.richter/certs/daemon.crt"
tls_key = "/Users/alberto/.richter/certs/daemon.key"

[mobile.pairing]
session_timeout_seconds = 120
max_sessions_per_hour = 10

[mobile.rate_limiting]
requests_per_minute_per_device = 60
requests_per_minute_per_ip = 300
websocket_max_per_device = 3
websocket_max_total = 20

[mobile.redaction]
command_output_max_chars = 2000
reasoning_trace_max_chars = 500
redact_secrets = true
redact_absolute_paths = true
```

### Mobile App Configuration

The mobile app's security configuration is set at build time and stored
securely:

- **Certificate pin**: SHA-256 fingerprint, stored in Keychain/Keystore.
  Set during pairing.
- **Timestamp tolerance**: ±30 seconds. Not configurable by user.
- **Biometric policy**: `BIOMETRIC_STRONG` (Android), `.deviceOwnerAuthenticationWithBiometrics` (iOS).
  Configurable: user can choose biometric + passcode fallback or
  biometric-only.

## Further Reading

- **[ADR 0006: Mobile Pairing and Trust Model](../adr/0006-mobile-pairing-and-trust-model.md)** — Formal trust model.
- **[ADR 0005: Mobile Companion Architecture](../adr/0005-mobile-companion-architecture.md)** — Architecture overview.
- **[ADR 0008: Local-First Mobile Remote Access](../adr/0008-local-first-mobile-remote-access.md)** — Relay security.
- **[Security](SECURITY.md)** — Mac-side security model.
- **[Privacy](PRIVACY.md)** — Privacy policy and data handling.
- **[Notifications](NOTIFICATIONS.md)** — Notification privacy design.
