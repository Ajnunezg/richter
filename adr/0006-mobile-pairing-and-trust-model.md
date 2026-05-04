# ADR 0006: Mobile Pairing and Trust Model

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter Mobile communicates with the Mac daemon over the local network (LAN
mode) or via an optional relay (remote mode). In either case, the mobile
device must authenticate to the daemon and the daemon must authenticate to
the mobile device. The daemon controls agent execution, has filesystem
access, and can run arbitrary commands. A compromised or impersonating
mobile client must not be able to approve dangerous actions or exfiltrate
data.

We need a trust model that:

- Does not rely on passwords (keylogged, phished, weak).
- Does not use reusable bearer tokens (leaked, stolen).
- Works over LAN (no internet required for pairing).
- Does not require an account system or OAuth provider.
- Supports multiple devices per user.
- Allows the user to revoke a device at any time.
- Provides forward secrecy for long-lived connections.
- Works across both iOS (Keychain) and Android (Keystore).
- Is simple enough that users can understand what's happening.

## Decision

Richter Mobile uses a **QR-based pairing ceremony** with **Ed25519 device
keypairs**, **single-use pairing tokens**, **TLS certificate pinning**, and
**scope-based authorization**.

### Pairing Ceremony

```
┌──────────────────┐                    ┌──────────────────────┐
│   Richter Mobile  │                    │   Richter Daemon      │
│   (React Native)  │                    │   (macOS)             │
└────────┬─────────┘                    └──────────┬───────────┘
         │                                         │
         │ 1. User clicks "Pair Mobile Device"      │
         │    in Mac menu bar                       │
         │                                         │
         │ 2. Daemon generates:                     │
         │    - pairing_token (32 random bytes)     │
         │    - pairing session (uuid, 120s TTL)    │
         │    - QR payload                          │
         │                                         │
         │ 3. Display QR code on screen             │
         │                                         │
         │ 4. Scan QR code                          │
         │    ───────────────┐                     │
         │                   │                     │
         │ 5. Parse QR payload:                     │
         │    { daemon_addr, port,                  │
         │      cert_fingerprint,                   │
         │      daemon_pubkey,                      │
         │      pairing_token }                     │
         │                   │                     │
         │ 6. HTTPS POST /mobile/pair               │
         │    ─────────────────────────────────→   │
         │    Body: { pairing_token,                │
         │            device_pubkey,                │
         │            device_name,                  │
         │            device_model }                │
         │                   │                     │
         │                   │ 7. Validate:         │
         │                   │    - pairing_token   │
         │                   │    - session not expired│
         │                   │    - session not used │
         │                   │                     │
         │                   │ 8. Register device:  │
         │                   │    - Assign device_id│
         │                   │    - Store pubkey    │
         │                   │    - Mark session used│
         │                   │    - Set capabilities│
         │                   │                     │
         │ 9. Receive device_id + capabilities      │
         │    ←─────────────────────────────────   │
         │                   │                     │
         │ 10. Store in Keychain/Keystore:          │
         │     - device_id                         │
         │     - device_keypair (private)           │
         │     - daemon_addr, port                 │
         │     - cert_fingerprint (pinned)          │
         │     - daemon_pubkey                     │
         │                   │                     │
         ▼                   ▼
     Pairing Complete    Device Registered
```

#### QR Payload Structure

```json
{
  "v": 1,
  "daemon_addr": "192.168.1.42",
  "daemon_port": 19557,
  "daemon_hostname": "albertos-macbook-pro.local",
  "cert_fingerprint_sha256": "a1b2c3d4e5f6...",
  "daemon_pubkey": "base64-encoded-ed25519-public-key",
  "pairing_token": "base64url-encoded-32-random-bytes",
  "expires_at": "2026-05-04T14:02:00Z"
}
```

The QR payload is ~300-400 bytes, well within QR code alphanumeric capacity
(4,296 chars for Version 40, or ~200 bytes for a phone-camera-friendly
Version 10). We use Version 10-15 QR codes to keep the QR scannable at a
distance.

#### Pairing Session Lifecycle

- **Creation**: User explicitly clicks "Pair Mobile Device" in the Mac UI.
  This is an intentional, attended action — pairing cannot be triggered
  programmatically.
- **Expiry**: 120 seconds. After expiry, the session is deleted and the
  pairing token is invalidated. The user must start a new pairing ceremony.
- **Single-use**: A pairing session can be used exactly once. After a
  successful pairing, the session is marked as used and cannot be reused.
- **Display**: The QR code is displayed only on the Mac's primary screen.
  No QR is sent over the network. The user must be physically present.
- **Rate limit**: Maximum 3 pairing attempts per 5 minutes. This prevents
  brute-force attacks on a brief pairing window (though the window is small
  enough that brute-forcing is already impractical).

### Device Authentication (Request Signing)

After pairing, every API request (except `/mobile/pair`) is authenticated
via an Ed25519 signature:

```
X-Richter-Device-Id: <device_id>
X-Richter-Signature: base64(ed25519_sign(device_private_key, signing_payload))
X-Richter-Timestamp: <unix_seconds>
```

The signing payload is:

```
<HTTP_METHOD>\n
<REQUEST_PATH>\n
<X-Richter-Timestamp>\n
<SHA256(request_body)>
```

For GET requests with no body, `SHA256("")` is used (the SHA-256 of an empty
string).

#### Replay Protection

- The daemon rejects requests with timestamps more than 30 seconds old
  (clock skew tolerance).
- The daemon maintains a bloom filter of seen `(device_id, timestamp,
  body_hash)` tuples within the 30-second window.
- The daemon requires device clock to be within 30 seconds of daemon clock.
  On first connection, the daemon sends its current time so the mobile app
  can detect clock drift.

#### Signature Verification

The daemon looks up the device's public key by `X-Richter-Device-Id`,
reconstructs the signing payload, and verifies the Ed25519 signature.
Failed verification returns 401 with no additional information (to avoid
oracle attacks).

### Scope-Based Authorization

Every device has a **capability set** assigned at pairing time and
configurable from the Mac UI:

| Capability | Default | Description |
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
| `admin:pair` | ❌ | Authorize new device pairing (reserved for Mac) |

Capabilities are enforced at the Mobile Gateway layer. A request that
requires a capability the device does not have returns 403 with a
descriptive error. The Mac UI shows each paired device with its current
capability set and allows the user to toggle capabilities (except `admin`,
which is Mac-only).

### TLS Requirements

All communication between the mobile app and the daemon is over HTTPS
(TLS 1.2 or 1.3) with the following requirements:

- **Daemon TLS identity**: The daemon generates a self-signed certificate
  at first launch, or the user can provide their own certificate (e.g.,
  from a local CA). The certificate fingerprint is included in the QR
  payload and pinned by the mobile app.
- **Certificate pinning**: The mobile app stores the daemon's certificate
  SHA-256 fingerprint in the Keychain/Keystore. On every connection, the
  mobile app verifies that the presented certificate matches the pinned
  fingerprint. Certificate changes require re-pairing.
- **No public CA**: The daemon's certificate is not publicly trusted. The
  mobile app pins the specific certificate, not a CA root. This avoids
  dependency on any CA infrastructure and works entirely offline.
- **mTLS consideration**: We considered mTLS (mutual TLS) where the device
  also presents a client certificate. We chose Ed25519 request signing over
  mTLS because: (a) it works identically over WebSocket connections,
  (b) request signing is easier to reason about and debug, and (c) it
  avoids platform-specific TLS client certificate quirks on iOS/Android.

### Revocation

Revocation is immediate and audit-logged:

- **Self-revoke**: The mobile user can revoke their own device from the
  mobile app (Settings → This Device → Revoke Access). This sends a signed
  `DELETE /mobile/revoke` request. The daemon immediately marks the device
  as revoked and logs the event.
- **Mac-side revoke**: The Mac user can revoke any device from the Mac UI
  (Settings → Mobile Devices → [device] → Revoke). This is effective
  immediately — the device's public key is deleted from the daemon's
  registry and all active connections from that device are terminated.
- **Revocation audit log**: Every revocation event is logged with the device
  ID, reason, timestamp, and the identity of the revoker (device self-revoke
  or Mac user).
- **No remote wipe**: Revocation only disconnects the device from the
  daemon. It does not remotely wipe the device. The mobile app's local cache
  of historical data persists on the device until the user uninstalls the
  app or clears app data. This is acceptable because the cached data is
  read-only history, not live agent control.

### Key Rotation

- **Device key rotation**: The mobile user can generate a new device keypair
  from Settings → This Device → Rotate Key. This requires re-authorization
  on the Mac side (the Mac UI shows a "Device is requesting key rotation"
  prompt). The old public key is replaced with the new one; the device ID
  remains the same.
- **Daemon key rotation**: If the daemon's Ed25519 keypair is rotated
  (which would happen if the user explicitly regenerates it), all paired
  devices must re-pair. This is an infrequent operation and a banner in the
  mobile app informs the user that re-pairing is required.
- **Certificate rotation**: If the daemon's TLS certificate changes (e.g.,
  the user replaces it with a new one), the certificate fingerprint changes.
  All paired devices must re-pair — the new fingerprint must be pinned.

### Secure Storage

| Platform | Storage Mechanism | Data Stored |
|---|---|---|
| iOS | Keychain (kSecClassKey) | Device Ed25519 private key |
| iOS | Keychain (kSecClassGenericPassword) | Daemon address, port, cert fingerprint, daemon pubkey, device ID |
| Android | Keystore (AndroidKeyStore provider) | Device Ed25519 private key |
| Android | EncryptedSharedPreferences | Daemon address, port, cert fingerprint, daemon pubkey, device ID |

On both platforms, the private key is generated inside the secure hardware
boundary when available (Secure Enclave on iOS, TEE-backed KeyStore on
Android) and never leaves it. Signing operations happen inside the secure
enclave.

Expo's `expo-secure-store` is used for non-key data (daemon address,
fingerprints, etc.), which wraps Keychain on iOS and EncryptedSharedPreferences
on Android.

### What We Explicitly Reject

- **Password-only auth**: Passwords can be keylogged, phished, brute-forced,
  and reused. No password-based authentication for mobile pairing.
- **Reusable bearer tokens**: A static API key or bearer token, once leaked
  (e.g., via a compromised device backup), grants permanent access. We use
  per-request Ed25519 signatures with timestamp-based replay protection.
- **OAuth / account system**: Requiring a cloud account to pair a device on
  the same LAN contradicts Richter's local-first philosophy. Pairing is
  device-to-device, initiated physically.
- **PIN-based pairing**: A short PIN (e.g., 6 digits) displayed on screen
  and entered on the mobile device is vulnerable to shoulder-surfing and
  brute-force (only 10^6 possibilities). QR codes encode 256 bits of entropy
  in the pairing token.
- **Unattended pairing**: Pairing requires the user to intentionally click
  "Pair Mobile Device" in the Mac UI. No programmatic pairing API. No
  headless pairing. This prevents a compromised agent from silently pairing
  a malicious device.

### Threat Model

| Threat | Mitigation |
|---|---|
| Attacker on LAN scans for daemon, tries to pair | Pairing requires QR code displayed on Mac screen; scanning requires physical presence |
| Attacker captures QR code (photo, screenshot) | Pairing token is single-use with 120s expiry; capturing the QR after the session is used or expired is worthless |
| Attacker impersonates daemon (spoofs Bonjour) | Certificate pinning: mobile app verifies SHA-256 fingerprint from QR payload against presented certificate |
| Attacker replays a captured signed request | Timestamp check (30s window) + bloom filter for replay detection |
| Stolen mobile device (unlocked) | Biometric requirement on approve/deny/cancel/revoke endpoints; remote revocation from Mac |
| Stolen mobile device (locked) | Device encryption (iOS/Android default); revocation from Mac |
| Compromised Mac (malware steals device keys) | Device private keys never leave mobile secure enclave; Mac only stores public keys |
| Mac user pairs a malicious device | Pairing requires intentional Mac-side action; Mac UI lists all paired devices with revoke button |
| Relay compromised | E2E encryption: relay cannot read command contents, logs, or event data. See ADR 0008. |

### Technology References

- [Ed25519 (RFC 8032)](https://datatracker.ietf.org/doc/html/rfc8032)
- [iOS Keychain Services](https://developer.apple.com/documentation/security/keychain_services)
- [Android Keystore System](https://developer.android.com/privacy-and-security/keystore)
- [expo-secure-store](https://docs.expo.dev/versions/latest/sdk/securestore/)
- [expo-local-authentication](https://docs.expo.dev/versions/latest/sdk/local-authentication/)
- [expo-crypto](https://docs.expo.dev/versions/latest/sdk/crypto/) (for Ed25519 via native module bridge)
- [react-native-zeroconf](https://github.com/balthazar/react-native-zeroconf)
- [Certificate pinning in React Native](https://reactnative.dev/docs/security#certificate-pinning)
- [OWASP Mobile Security Testing Guide](https://mas.owasp.org/)

## Consequences

### Positive

- QR-based pairing requires physical presence, preventing remote pairing
  attacks.
- Ed25519 per-request signatures eliminate reusable credential risks.
- Certificate pinning prevents MITM even on hostile LANs.
- Scope-based capabilities allow users to restrict what each device can do.
- Immediate revocation with audit logging provides full control and
  accountability.
- Secure Enclave / TEE-backed key storage protects device keys from
  extraction.
- No passwords, no accounts, no OAuth — consistent with Richter's
  local-first philosophy.

### Negative

- QR-based pairing requires the user to be physically at their Mac, which
  is intentional but could be inconvenient if the user wants to pair while
  remote (e.g., via relay). Mitigation: if the relay is already set up and
  a device was previously paired, re-pairing is not needed for relay access.
- Signature-based auth adds ~1-2ms overhead per request for Ed25519 signing
  and verification. Acceptable given request volumes.
- Certificate pinning means certificate changes require all devices to
  re-pair. This is documented clearly.
- No password-based fallback means if QR scanning fails (e.g., damaged
  camera), there is no alternative pairing method. Mitigation: manual
  entry of the pairing token (displayed alongside the QR code as a fallback)
  is supported.
- Bloom filter for replay detection has a negligible false-positive rate;
  a legitimate request that collides would be rejected and must be retried
  with a new timestamp.
