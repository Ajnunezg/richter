# ADR 0008: Local-First Mobile Remote Access Design

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter Mobile communicates with the Mac daemon. By default, this communication
happens over the local network (LAN). But users are not always on the same LAN
as their Mac — they might be at a coffee shop, commuting, or traveling.

We need to decide: does Richter Mobile support remote access, and if so, how?

Richter's core philosophy is local-first, cloud-optional. Any remote access
design must respect this:

- No cloud dependency by default.
- No vendor lock-in.
- No plaintext data passing through third-party infrastructure.
- The user must explicitly opt in.
- The user must be able to self-host any relay infrastructure.

## Decision

Richter Mobile supports **three connectivity modes**, with LAN as the default
and relay as an opt-in enhancement:

### Mode 1: LAN (Default, Zero Configuration)

```
Mobile App ──HTTPS/WSS──→ richterd Mobile Gateway
                  │
        Discovery via Bonjour (iOS) / NSD (Android)
        Daemon advertises _richter._tcp on port 19557
        Certificate pinned from pairing QR code
```

- **No internet required.** Works on any local network where the Mac and
  mobile device can reach each other.
- **Zero configuration.** Bonjour/NSD discovery means the mobile app
  automatically finds the daemon without manual IP entry.
- **Always enabled.** LAN mode cannot be disabled; it's the baseline.
- **Fallback**: Manual IP:port entry for networks where mDNS is blocked or
  unreliable (corporate networks, some VPNs).

**Local network privacy considerations**:

- **iOS**: iOS 14+ shows a "Local Network" permission dialog the first time
  the app attempts Bonjour discovery. The app must include the
  `NSLocalNetworkUsageDescription` and `NSBonjourServices` keys in
  `Info.plist`. If the user denies, the app falls back to manual IP:port
  entry.
- **Android**: Android 13+ requires the `NEARBY_WIFI_DEVICES` permission
  for NSD. Android 12 and below use `ACCESS_FINE_LOCATION` (because NSD
  can leak approximate location via WiFi SSIDs; Richter only uses it for
  local service discovery).

### Mode 2: Manual / VPN

```
Mobile App ──HTTPS/WSS──→ richterd Mobile Gateway
                  │
        User manually enters IP:port or hostname
        Or: connect via VPN (Tailscale, WireGuard, ZeroTier)
        Same TLS + request signing as LAN mode
```

- **Manual entry**: The user enters the daemon's IP address and port
  directly. Useful when Bonjour/NSD is unavailable but the Mac is reachable
  (e.g., corporate network, VPN without mDNS forwarding).
- **VPN**: If the user's mobile device and Mac are on the same VPN (e.g.,
  Tailscale, WireGuard), the daemon is reachable via its VPN IP. This is
  the recommended approach for remote access when the user already runs a
  VPN — no additional infrastructure needed.
- **No special handling**: The mobile app doesn't know or care whether the
  IP is a LAN address, a manually entered address, or a VPN address. It's
  just an HTTPS/WSS connection with the pinned certificate.

### Mode 3: Optional Relay (Opt-in, Self-Hosted)

```
Mobile App ──HTTPS/WSS──→ Richter Relay ──HTTPS/WSS──→ richterd
                  │              │
          E2E encrypted    Relay sees only:
          tunnel           - Connection metadata
                           - Packet timing
                           - IP addresses
                           Relay CANNOT read:
                           - Command contents
                           - Event data
                           - Repo names
                           - Log output
                           - Search queries
```

The relay is a **transparent WebSocket proxy** with the following properties:

- **Opt-in**: The user must explicitly set up and configure the relay.
  Nothing in Richter Mobile requires the relay.
- **Self-hosted first**: The relay is distributed as a single static binary
  and a Docker Compose file. Users deploy it on their own infrastructure
  (a VPS, a home server, a Raspberry Pi).
- **Managed relay later**: A managed relay service may be offered in the
  future as a convenience, but the self-hosted option will always exist.
- **E2E encrypted**: The mobile app and daemon communicate over their
  existing TLS connection (certificate pinned). The relay sees TLS-encrypted
  traffic. Application-layer encryption (Ed25519 request signing) provides
  an additional layer: even if TLS were broken, individual request contents
  would still be opaque.
- **Metadata minimization**: The relay sees only connection-level metadata:
  source IP, destination IP, packet timing, connection duration. It does
  not see command names, log output, repo names, event content, or search
  queries. These are all inside the encrypted tunnel.
- **No state**: The relay stores no data beyond connection logs (for
  debugging). No event data, no run history, no user data of any kind
  persists on the relay.
- **No auth at relay**: Authentication happens between the mobile app and
  daemon (Ed25519 request signing). The relay does not authenticate clients —
  it proxies the encrypted tunnel. An attacker who connects to the relay
  cannot authenticate to the daemon without a valid device keypair.

#### Relay Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                      Richter Relay                            │
│                                                               │
│  ┌─────────────────┐     ┌─────────────────┐                 │
│  │  Mobile-facing   │     │  Daemon-facing   │                 │
│  │  HTTPS/WSS       │────→│  HTTPS/WSS       │                 │
│  │  endpoint        │     │  connection       │                 │
│  │  :443 (public)   │     │  :19557 (daemon)  │                 │
│  └─────────────────┘     └─────────────────┘                 │
│           │                        │                          │
│           │  TLS termination       │  TLS origination         │
│           │  (relay's own cert)    │  (pinned daemon cert)    │
│           │                        │                          │
│           └──────────┬─────────────┘                          │
│                      │                                        │
│           ┌──────────┴──────────┐                             │
│           │  Connection mapper   │                             │
│           │  daemon_id → backend │                             │
│           └─────────────────────┘                             │
│                                                               │
│  Each daemon registers with a unique ID.                      │
│  Mobile client connects with daemon_id in the URL path.       │
│  Relay maps daemon_id → daemon's backend connection.           │
│  All payload data is opaque to the relay.                     │
└──────────────────────────────────────────────────────────────┘
```

**Relay URL scheme**: `wss://relay.example.com/daemon/<daemon_id>/ws`

The daemon_id is a random identifier generated by the daemon at first launch.
It is included in the QR pairing payload so the mobile app knows it. The
daemon_id is not secret (it's effectively a routing key), but it is not
broadcast — only paired devices know it.

#### Self-Hosted Relay Setup

```bash
# Download the relay binary
curl -L https://github.com/richter/relay/releases/latest/download/richter-relay-linux-amd64 -o richter-relay
chmod +x richter-relay

# Or use Docker Compose
# docker-compose.yml:
#   services:
#     relay:
#       image: ghcr.io/richter/relay:latest
#       ports:
#         - "443:443"
#       volumes:
#         - ./certs:/certs:ro
#       environment:
#         - RELAY_TLS_CERT=/certs/fullchain.pem
#         - RELAY_TLS_KEY=/certs/privkey.pem
#         - RELAY_MAX_CONNECTIONS=100
#         - RELAY_LOG_LEVEL=info

docker compose up -d
```

The daemon connects to the relay as an outbound WebSocket client (so no
inbound firewall rules needed on the Mac). The mobile app connects to the
relay as a WebSocket client. The relay bridges the two connections.

### Push Notification Privacy

See **[NOTIFICATIONS.md](../docs/NOTIFICATIONS.md)** for the full notification
design. Key privacy points relevant to remote access:

- **Push notifications are opt-in.** The user must explicitly configure a
  push provider (APNs/FCM) in the daemon. Without this, notifications are
  local-only (app in foreground) or delivered via background WebSocket
  polling (limited by iOS/Android background constraints).
- **Generic payloads**: Push notification payloads contain only generic
  metadata: "Agent action requires approval" or "Build failed in <repo>".
  No command output, no log content, no file paths appear in push payloads.
- **Content fetched after open**: When the user taps a notification, the
  app opens, connects to the daemon (LAN or relay), and fetches the actual
  event content. The push notification is a signal, not the content.
- **Relay does not see push payloads**: The push notification pathway
  (daemon → APNs/FCM → device) is separate from the relay pathway
  (device → relay → daemon). The relay never handles push notifications.

### iOS / Android Background Constraints

Both platforms restrict background network activity:

| Constraint | iOS | Android |
|---|---|---|
| Background WebSocket | Suspended after ~30s when app is backgrounded | Can run in foreground service |
| Background fetch | `BGAppRefreshTask`, system-scheduled, ~15s window | `WorkManager`, periodic, ~10 min minimum interval |
| Push notifications | Full support via APNs | Full support via FCM |
| Local network discovery | Not available in background | Not available in background |

**Design implications**:

- The mobile app maintains a WebSocket connection while in the foreground
  for real-time event streaming.
- When backgrounded, the app relies on push notifications (if configured)
  or periodic background fetch (every ~15 minutes minimum on iOS, ~10
  minutes on Android) to check for important events.
- Bonjour/NSD discovery runs only when the app is in the foreground and
  the user is on the connectivity screen.
- The daemon pushes notification-worthy events via APNs/FCM if configured.
  If not configured, events are queued and delivered on next foreground
  connection.
- On Android, a foreground service with a persistent notification can
  maintain the WebSocket connection in the background. This is optional
  and user-configurable (some users prefer real-time updates; others
  prefer battery life).

### Remote Access Without Relay

The relay is not the only way to achieve remote access:

| Method | Setup Complexity | Reliability | Battery Impact |
|---|---|---|---|
| **LAN only** | None | Excellent on LAN, none off LAN | None |
| **VPN (Tailscale/WireGuard)** | Low-Medium (install VPN) | Excellent | Low (VPN overhead) |
| **Manual IP:port (port forwarding)** | Medium (router config) | Moderate (IP changes) | None |
| **Self-hosted relay** | Medium (deploy relay) | Excellent | Low |
| **Managed relay** | Low (sign up for service) | Excellent | Low |

VPN is the recommended remote access method for users who already use a VPN
(Tailscale, WireGuard, ZeroTier, etc.). The relay exists for users who want
remote access without running a VPN or configuring port forwarding.

### No Cloud Dependency by Default

Richter Mobile ships with LAN mode only. The relay is not bundled, not
required, and not auto-configured. The user must:

1. Decide they want remote access.
2. Choose a method (VPN, relay, port forwarding).
3. Configure it themselves.
4. Enter the relay address (or VPN IP) in the mobile app settings.

This is consistent with Richter's overall philosophy: local-first, no
accounts, no cloud database, no vendor lock-in. The relay is a convenience
for users who want it, not a dependency for the product to function.

### Technology References

- [iOS Bonjour / Local Network Privacy](https://developer.apple.com/documentation/network/bonjour)
- [Android NSD](https://developer.android.com/training/connect-devices-wirelessly/nsd)
- [iOS Background Execution](https://developer.apple.com/documentation/backgroundtasks)
- [Android Background Execution Limits](https://developer.android.com/guide/components/foreground-services)
- [Tailscale](https://tailscale.com/)
- [WireGuard](https://www.wireguard.com/)
- [WebSocket protocol (RFC 6455)](https://datatracker.ietf.org/doc/html/rfc6455)
- [Apple Push Notification service (APNs)](https://developer.apple.com/documentation/usernotifications)
- [Firebase Cloud Messaging (FCM)](https://firebase.google.com/docs/cloud-messaging)
- [expo-notifications](https://docs.expo.dev/versions/latest/sdk/notifications/)

## Consequences

### Positive

- LAN mode is zero-configuration, works entirely offline, and is the
  default. No cloud dependency.
- VPN mode leverages existing infrastructure many users already have.
- Relay is fully self-hostable — no vendor lock-in.
- E2E encryption means the relay cannot read any application data.
- Push notification privacy is preserved via generic payloads and
  content-fetch-after-open.
- Clear separation: relay proxies encrypted tunnels; it is not an
  application server.

### Negative

- LAN-only default means no remote access out of the box. Users who expect
  cloud-connected mobile apps may be confused.
- Self-hosting a relay requires some infrastructure knowledge (though the
  Docker Compose file makes it straightforward).
- iOS background constraints mean real-time updates without push
  notifications are impossible when the app is backgrounded. Push
  requires APNs configuration.
- Bonjour/NSD discovery can be unreliable on some networks (corporate,
  guest WiFi, certain router configurations). The manual IP fallback adds
  friction.
- The relay adds infrastructure to maintain and secure (even if
  self-hosted by the user).

### Mitigations

- Clear documentation in the app and user guide explaining connectivity
  modes and tradeoffs.
- One-click setup guide for Tailscale as the recommended VPN approach.
- Docker Compose relay setup with detailed, step-by-step instructions
  and a configuration validation tool.
- The manual IP:port entry screen includes a "Copy from Clipboard"
  button and validates the address format.
- The app gracefully degrades when connectivity is lost: shows last-known
  state with a "Last updated X minutes ago" banner.
- Push notification setup guide walks the user through APNs/FCM
  configuration (only needed if they want push).
