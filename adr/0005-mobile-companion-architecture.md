# ADR 0005: Mobile Companion Architecture

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter's macOS app provides a menu-bar dashboard and full-window interface
for monitoring and controlling AI coding agents. But the user is not always
at their Mac. They might be in a meeting, on a walk, or away from their desk
when a test suite fails, a build breaks, or an agent gets stuck in a loop
and needs human approval.

We need a mobile companion app that lets the user:

- See what their agents are doing right now (the Now view)
- Approve or deny agent actions that require human sign-off
- Browse recent run history and see results at a glance
- Receive push notifications for truly important events
- Do all of this without any cloud dependency by default

The mobile app is a **companion**, not a second orchestrator. It does not
initiate runs, manage workspace configuration, or execute commands. The Mac
daemon (`richterd`) remains the sole source of truth and the sole
orchestrator. The mobile app is a read-heavy surface with a narrow write
path limited to approvals and cancellations.

We need to decide: what is the mobile app's architecture, what framework do
we use, how does it communicate with the Mac, and what is the trust model?

## Decision

Richter Mobile uses a **four-component mobile architecture**:

1. **Richter Mobile App** — React Native + Expo (development builds), one
   codebase for iOS and Android. The primary mobile user surface.
2. **Mobile Gateway** — A new daemon module (`richterd mobile-gateway`) that
   runs inside the existing Rust daemon. Exposes a scoped HTTP + WebSocket
   API surface specifically for mobile clients.
3. **Daemon Core** — The existing `richterd` orchestration engine. Unchanged.
   The Mobile Gateway is a daemon module, not a separate process.
4. **Optional Relay** — A self-hosted relay service for connectivity when the
   mobile device is not on the same LAN as the Mac. Opt-in, E2E encrypted.

```
┌──────────────────────────────────────────────────────────────────┐
│                         User's Mac                                │
│                                                                    │
│  ┌──────────────────────────────────────────────────────────────┐ │
│  │                   Richter Daemon (richterd)                    │ │
│  │                                                                │ │
│  │  ┌────────────────────┐  ┌────────────────────────────────┐  │ │
│  │  │  Daemon Core       │  │  Mobile Gateway (new module)    │  │ │
│  │  │  - classifier      │  │  - HTTP API (scoped)            │  │ │
│  │  │  - fingerprint     │  │  - WebSocket / SSE              │  │ │
│  │  │  - scheduler       │  │  - Pairing session mgr          │  │ │
│  │  │  - run mgr         │  │  - Device registry              │  │ │
│  │  │  - ...             │  │  - Scope enforcement            │  │ │
│  │  └────────┬───────────┘  └───────────────┬────────────────┘  │ │
│  │           │                               │                    │ │
│  └───────────┼───────────────────────────────┼────────────────────┘ │
│              │                               │                       │
│              │  Internal daemon IPC          │  HTTPS + WSS          │
│              │                               │  (port from config)   │
└──────────────┼───────────────────────────────┼───────────────────────┘
               │                               │
               │                               │  LAN: mDNS/Bonjour/NSD
               │                               │  Remote: optional relay
               │                               │
        ┌──────┴──────┐                 ┌──────┴──────────┐
        │  richter CLI │                 │  Richter Mobile  │
        │  (unchanged) │                 │  (React Native)  │
        └─────────────┘                 └─────────────────┘
```

### Why React Native + Expo Development Builds

See **[ADR 0007](0007-why-react-native-expo.md)** for the full analysis.
In summary:

| Criteria | React Native + Expo | SwiftUI + Kotlin/Compose | Flutter | KMP | PWA |
|---|---|---|---|---|---|
| Shared codebase | ✅ TypeScript, iOS+Android | ❌ Two codebases | ✅ Dart | ⚠️ Shared logic only | ✅ Web |
| Native module support | ✅ | ✅ | ✅ | ✅ | ❌ |
| Bonjour/NSD access | ✅ react-native-zeroconf | ✅ | ⚠️ Limited | ✅ | ❌ |
| Secure storage | ✅ expo-secure-store | ✅ | ✅ | ✅ | ❌ |
| Biometric auth | ✅ expo-local-authentication | ✅ | ✅ | ✅ | ⚠️ WebAuthn only |
| Push notifications | ✅ | ✅ | ✅ | ✅ | ⚠️ Limited |
| OTA updates | ✅ EAS Update | ❌ | ✅ | ❌ | ✅ |
| Ecosystem maturity | ✅ Very mature | ✅ Mature | ⚠️ Growing | ⚠️ Early | N/A |
| Team velocity | ✅ Single team, one language | ⚠️ Two teams | ⚠️ Dart gap | ⚠️ Requires native UI | ✅ |

React Native with Expo development builds gives us native iOS and Android
from one TypeScript codebase without sacrificing Bonjour/NSD discovery,
secure storage, biometric auth, or push notifications — all of which are
hard requirements. Expo development builds (vs. Expo Go) let us use native
modules like `react-native-zeroconf` and `expo-secure-store` without ejecting.

### Communication Model

The mobile app communicates with the Mac daemon over LAN by default,
with an optional relay for remote access.

#### LAN Mode (Default)

```
Mobile App ──HTTPS/WSS──→ richterd Mobile Gateway (LAN IP:port)
                                │
                     mDNS/Bonjour/NSD discovery
                     (advertise _richter._tcp)
```

- The daemon's Mobile Gateway binds to a configurable port (default: 19557).
- The daemon advertises via Bonjour (macOS) / NSD (Android).
- The mobile app discovers the daemon on the local network.
- All communication is over HTTPS/WSS with the daemon's self-signed or
  user-provided TLS certificate.
- The mobile app pins the daemon's certificate on first pairing.

#### Remote Mode (Opt-in)

```
Mobile App ──HTTPS/WSS──→ Self-Hosted Relay ──HTTPS/WSS──→ richterd
                                │
                        E2E encrypted tunnel
                        Relay sees only metadata
                        (IP, timing, connection duration)
```

See **[ADR 0008](0008-local-first-mobile-remote-access.md)** for the full
remote access design.

### Why Companion, Not Second Orchestrator

The mobile app is deliberately constrained:

| Capability | Mobile App | Mac App (Primary) |
|---|---|---|
| View agent status (Now) | ✅ | ✅ |
| Browse run history | ✅ | ✅ |
| Approve/deny agent actions | ✅ | ✅ |
| Cancel a running agent | ✅ | ✅ |
| Search events and runs | ✅ | ✅ |
| View repo health | ✅ | ✅ |
| Initiate agent runs | ❌ | ✅ |
| Configure workspaces | ❌ | ✅ |
| Manage integrations | ❌ | ✅ |
| Manage LLM model routing | ❌ | ✅ |
| Shell execution | ❌ | ✅ |
| File system access | ❌ | ✅ |

The mobile app is a **read-heavy companion** with a narrow write surface
(approvals, cancellations). It never initiates runs, never touches the
filesystem, never manages workspace configuration. This constraint is
architectural, not temporary — it simplifies the trust model, reduces
attack surface, and avoids the need for the mobile device to maintain
state that could drift from the Mac.

### Pairing and Trust

See **[ADR 0006](0006-mobile-pairing-and-trust-model.md)** for the complete
trust model. Summary:

1. User initiates pairing from the Mac menu bar: "Pair Mobile Device."
2. Daemon generates a single-use pairing session (120-second expiry).
3. Daemon displays a QR code containing: daemon LAN address, port, TLS
   certificate fingerprint, pairing session token, and daemon public key.
4. Mobile app scans the QR code.
5. Mobile app connects to daemon over HTTPS, presents the pairing token.
6. Daemon validates the token, registers the device.
7. Mobile app generates a device Ed25519 keypair, registers the public key
   with the daemon.
8. Mobile app stores daemon info (address, certificate fingerprint, public
   key) in iOS Keychain / Android Keystore.
9. Daemon stores device public key and scoped capabilities.
10. All subsequent communication is authenticated via Ed25519 signatures
    over request bodies + timestamps.

### Local-First, Cloud-Optional

Richter Mobile follows the same local-first philosophy as the rest of
Richter. See **[ADR 0008](0008-local-first-mobile-remote-access.md)**.

- **No cloud database.** The daemon is the source of truth. The mobile app
  caches data locally (AsyncStorage or SQLite) for offline viewing, but the
  daemon is authoritative.
- **No cloud auth.** Pairing is device-to-device over LAN. No accounts,
  no passwords, no OAuth.
- **No cloud relay by default.** LAN mode works entirely offline. Relay is
  opt-in and self-hosted.
- **No telemetry by default.** The mobile app sends no analytics to any
  third party unless the user opts in.

### Mobile Gateway API Surface

The Mobile Gateway exposes a scoped HTTP + WebSocket API:

| Endpoint | Method | Auth | Purpose |
|---|---|---|---|
| `/mobile/status` | GET | Device signature | Daemon health, agent summary |
| `/mobile/now` | GET | Device signature | Current agent activity |
| `/mobile/runs` | GET | Device signature | Paginated run history |
| `/mobile/runs/:id` | GET | Device signature | Single run detail |
| `/mobile/repos` | GET | Device signature | Repo list + health |
| `/mobile/events` | GET | Device signature | Recent events (filtered) |
| `/mobile/search` | POST | Device signature | Full-text search |
| `/mobile/approve` | POST | Device signature + biometric | Approve pending action |
| `/mobile/deny` | POST | Device signature + biometric | Deny pending action |
| `/mobile/cancel` | POST | Device signature + biometric | Cancel running agent |
| `/mobile/pair` | POST | Pairing token (single-use) | Initial device registration |
| `/mobile/revoke` | DELETE | Device signature + biometric | Self-revoke this device |
| `/mobile/ws` | WSS upgrade | Device signature | Real-time event stream |

All endpoints except `/mobile/pair` require a valid device signature header.
The `/mobile/approve`, `/mobile/deny`, `/mobile/cancel`, and `/mobile/revoke`
endpoints additionally require biometric confirmation on the device (iOS
LocalAuthentication / Android BiometricPrompt) before the request is signed
and sent.

### Technology References

- [React Native documentation](https://reactnative.dev/docs/getting-started)
- [Expo development builds](https://docs.expo.dev/develop/development-builds/introduction/)
- [EAS Build](https://docs.expo.dev/build/introduction/)
- [iOS Local Network Privacy](https://developer.apple.com/documentation/network/bonjour)
- [Android NSD](https://developer.android.com/training/connect-devices-wirelessly/nsd)
- [react-native-zeroconf](https://github.com/balthazar/react-native-zeroconf)
- [expo-secure-store](https://docs.expo.dev/versions/latest/sdk/securestore/)
- [expo-local-authentication](https://docs.expo.dev/versions/latest/sdk/local-authentication/)
- [expo-notifications](https://docs.expo.dev/versions/latest/sdk/notifications/)

## Consequences

### Positive

- One TypeScript codebase for iOS and Android maximizes team velocity.
- Expo development builds give full native module access without ejecting.
- Mobile Gateway as a daemon module reuses existing Rust infrastructure.
- Local-first design means no cloud dependency, no account management,
  no vendor lock-in.
- Companion-only model drastically simplifies the mobile trust model.
- Certificate pinning + Ed25519 device keys provide strong authentication
  without passwords or OAuth.

### Negative

- React Native adds a runtime dependency and bridging overhead vs. native.
- Bonjour/NSD discovery has platform-specific quirks and limitations.
- LAN-only mode by default means no connectivity when away from the local
  network without setting up a relay or VPN.
- Mobile Gateway increases daemon attack surface (network-exposed service).
- React Native + Expo version upgrades can be painful (though Expo's
  managed workflow mitigates this).
- The mobile app cannot operate independently of the Mac daemon — no
  standalone mode.

### Mitigations

- Expo's config plugins and EAS Build handle native module integration
  without manual iOS/Android project management.
- Bonjour/NSD discovery is wrapped in a cross-platform abstraction layer
  with fallback to manual IP:port entry.
- Relay is well-documented and easy to self-host (Docker Compose,
  single binary).
- Mobile Gateway runs on a separate port with strict TLS, request
  validation, and rate limiting.
- Expo's upgrade helper and `npx expo-doctor` streamline version upgrades.
- The companion constraint is documented clearly in the user guide and
  reflected in the API surface (no run-initiation endpoints).
