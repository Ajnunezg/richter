# Richter Mobile

## What Richter Mobile Is

Richter Mobile is the companion app for Richter. It runs on your iPhone or
Android phone and connects to the Richter daemon (`richterd`) running on
your Mac. It lets you:

- **See what your AI agents are doing right now** — which agents are
  running, what commands they're executing, and what results they're
  producing.
- **Approve or deny agent actions** — when an agent needs human sign-off
  before proceeding, you can approve or deny from your phone.
- **Browse run history** — review past agent runs, see what passed and
  what failed, and inspect event details.
- **Search across your work** — find specific runs, events, errors, or
  commands across all your repositories.
- **Get notified of important events** — push notifications for failures,
  approvals needed, and other high-priority events.

## What Richter Mobile Is Not

Richter Mobile is a **companion**, not a replacement for the Mac app.
It does not:

- Initiate agent runs (you start runs from your Mac or via `richter` CLI).
- Configure workspaces, integrations, or model routing.
- Execute commands or access the filesystem.
- Serve as a standalone app — it requires a connection to the Mac daemon.

The Mac daemon is the orchestrator. The mobile app is a monitoring and
approval surface.

## Architecture Overview

```
┌─────────────────┐         ┌──────────────────────────────┐
│  Richter Mobile  │  HTTPS  │  Richter Daemon (richterd)    │
│  (React Native)  │◄───────►│  ┌──────────────────────────┐│
│                  │  WSS    │  │  Mobile Gateway module    ││
│  iOS + Android   │         │  │  - Scoped HTTP API        ││
│  TypeScript      │         │  │  - WebSocket event stream ││
│                  │         │  │  - Device registry        ││
│                  │         │  │  - Scope enforcement      ││
│                  │         │  └──────────────────────────┘│
│                  │         │              │               │
│                  │         │    Daemon Core (unchanged)   │
│                  │         │    - Agent orchestration     │
│                  │         │    - Run management          │
│                  │         │    - Event classification    │
└─────────────────┘         └──────────────────────────────┘
```

For the full architecture decision, see **[ADR 0005](../adr/0005-mobile-companion-architecture.md)**.

### Technology Stack

| Layer | Technology |
|---|---|
| Framework | React Native with Expo development builds |
| Language | TypeScript |
| Navigation | expo-router (file-based routing) |
| State management | Zustand |
| Secure storage | expo-secure-store (Keychain / Keystore) |
| Biometrics | expo-local-authentication |
| Push notifications | expo-notifications |
| Network discovery | react-native-zeroconf (Bonjour / NSD) |
| Local cache | react-native-mmkv |
| Crypto | Ed25519 via native module bridge |
| CI/CD | EAS Build + EAS Update |

For the framework selection rationale, see **[ADR 0007](../adr/0007-why-react-native-expo.md)**.

## Getting Started

### Prerequisites

- **Richter daemon (`richterd`) version 1.x or later** running on your Mac
  (macOS 14+).
- **iOS 16+** or **Android 13+**.
- Your Mac and mobile device must be on the same local network (for initial
  pairing; remote access can be configured later).

### Install

#### iOS (TestFlight)

1. Join the Richter Mobile TestFlight beta (invite link in your Richter
   welcome email or from the Mac app Settings → Mobile).
2. Install Richter Mobile from TestFlight.
3. Open the app. You'll see the pairing screen.

#### Android (Google Play Internal Testing)

1. Join the Richter Mobile internal testing track (invite link from the
   Mac app Settings → Mobile).
2. Install Richter Mobile from Google Play.
3. Open the app. You'll see the pairing screen.

#### Development Build

If you're building from source:

```bash
cd mobile
npm install
npx expo prebuild
npx expo run:ios    # or npx expo run:android
```

### Pair with Your Mac

Pairing is a one-time ceremony that establishes a secure, authenticated
connection between your mobile device and your Mac's daemon.

1. **On your Mac**: Open Richter in the menu bar, click the gear icon
   (Settings), then select **Mobile Devices** → **Pair New Device**.
   A QR code appears on your screen.

2. **On your phone**: In Richter Mobile, tap **Pair with Mac**. The camera
   opens. Point it at the QR code on your Mac's screen.

3. The app scans the QR code and connects to your daemon. Within seconds,
   you'll see the pairing confirmation.

4. **Name your device** (e.g., "Alberto's iPhone") so you can identify it
   later in the Mac settings.

5. Done! The app shows the Now view with your agents' current status.

**Fallback**: If your camera doesn't work, tap "Enter Pairing Code
Manually" and type the code displayed below the QR code on your Mac.

**Troubleshooting**: See [Pairing Troubleshooting](#pairing-troubleshooting).

For the complete trust model, see **[ADR 0006](../adr/0006-mobile-pairing-and-trust-model.md)**
and **[Mobile Security](MOBILE_SECURITY.md)**.

### Connect

After pairing, the app automatically connects to your daemon whenever both
are on the same local network. The connection indicator in the top-right
shows your connectivity status:

- **🟢 Connected (LAN)** — Connected directly over your local network.
- **🔵 Connected (Relay)** — Connected via an optional relay (remote access).
- **🔵 Connected (VPN)** — Connected via a VPN (Tailscale, WireGuard, etc.).
- **🟡 Connecting** — Trying to reach the daemon.
- **🔴 Disconnected** — Cannot reach the daemon. Check that your Mac is on
  and on the same network.

If the app doesn't connect automatically, go to **Settings** → **Connectivity**
and tap **Scan for Daemon** to re-discover the daemon on your network.

## Navigation

Richter Mobile has six main screens, accessible from the bottom tab bar:

### Now

The home screen. Shows everything happening right now:

- **Active agents** — Which AI coding agents are currently running, with
  their status (running, waiting, stuck).
- **Current commands** — What each agent is executing and the latest output
  line.
- **Resource usage** — CPU and memory utilization across all running agents.
- **Pending approvals** — A counter badge if any agent is waiting for your
  approval. Tap to jump to the Approvals screen.
- **Recent events** — The last few important events, with color-coded
  severity indicators.

Pull down to refresh. The Now view updates in real time via WebSocket while
you're on this screen.

### Repos

A list of all repositories the daemon is monitoring:

- **Repo health summary** — Each repo shows: last run status (pass/fail),
  number of active agents, and a mini sparkline of recent run outcomes.
- **Tap a repo** for detail: recent runs, configured agents, workspace path.
- **Search** — The search bar at the top searches across all repos and runs.

### Runs

A chronological list of agent runs across all repos:

- **Filter** by repo, agent type, status (passed, failed, cancelled,
  running), or time range.
- **Sort** by time (newest first) or by importance (failures first).
- **Tap a run** for full detail: all events, command output, error logs,
  and a timeline visualization.
- **Swipe left** on a run to cancel it (if still running).

### Agents

A view of all configured agents across your repos:

- **Agent type** — Codex, Claude Code, Droid, Forge Code, Kimi, MiniMax, etc.
- **Status** — Active, idle, errored, or disconnected.
- **Tap an agent** for: recent runs, configuration summary, and the option
  to view detailed logs.

### Approvals

Shows all agent actions that are waiting for your approval:

- **What needs approval** — The agent, the action it wants to take, the
  reasoning it provided, and the risk level.
- **Approve** — Green button. Requires biometric confirmation (Face ID,
  Touch ID, or fingerprint). The action proceeds.
- **Deny** — Red button. Requires biometric confirmation. The action is
  blocked and the agent is informed.
- **Auto-deny timer** — If the daemon has a timeout configured for
  approvals, the remaining time is shown. Approvals that time out are
  automatically denied.

Biometric confirmation on approve/deny ensures that someone with physical
access to your unlocked phone cannot approve dangerous actions.

### Search

Full-text search across runs, events, errors, and commands:

- **Search operators**:
  - `repo:<name>` — Filter to a specific repo.
  - `agent:<type>` — Filter to agent type (codex, claude, droid, etc.).
  - `status:<passed\|failed\|running>` — Filter by run status.
  - `error:<text>` — Search error messages.
  - `before:<date>` / `after:<date>` — Time range filter.
- **Recent searches** shown below the search bar.
- **Results** show run summaries with highlighted match snippets.
- Tap a result to jump to the run detail.

### Settings

Accessible from the gear icon or the Settings tab:

- **This Device** — Device name, pairing status, public key fingerprint,
  revoke access, rotate key.
- **Connectivity** — Current connection mode, daemon discovery, relay
  configuration, VPN settings.
- **Notifications** — Notification preferences, importance thresholds,
  quiet hours. See [Notifications](NOTIFICATIONS.md).
- **Appearance** — System / Light / Dark mode.
- **About** — Version, licenses, documentation links.

## Pairing Flow

The pairing flow establishes trust between your mobile device and the Mac
daemon. It is a one-time process per device.

```
┌──────────┐                         ┌──────────┐
│  Mobile   │                         │   Mac     │
│  Device   │                         │  Daemon   │
└─────┬─────┘                         └─────┬─────┘
      │                                      │
      │   1. User clicks "Pair Mobile"        │
      │      on Mac                           │
      │                                      │
      │   2. Daemon generates:                │
      │      - Pairing token (random)         │
      │      - Pairing session (120s TTL)     │
      │      - QR code                        │
      │                                      │
      │   3. QR code displayed on screen      │
      │      ◄────────────────────────────    │
      │                                      │
      │   4. Mobile scans QR                  │
      │                                      │
      │   5. Mobile connects via HTTPS        │
      │      Presents pairing token           │
      │      ─────────────────────────────►   │
      │                                      │
      │   6. Daemon validates token           │
      │      Registers device                 │
      │                                      │
      │   7. Mobile generates Ed25519 keypair │
      │      Sends public key                 │
      │                                      │
      │   8. Pairing complete                 │
      │      Device authorized                │
      │      ◄────────────────────────────    │
      │                                      │
      ▼                                      ▼
```

### Re-pairing

You need to re-pair if:

- You revoke the device and want to add it back.
- The daemon's TLS certificate changes (rare).
- The daemon's keypair is rotated (very rare).
- You get a new phone and want to transfer your pairing (pair the new phone
  as a new device; revoke the old one separately).

### Multiple Devices

You can pair multiple mobile devices with the same Mac daemon. Each device
gets its own Ed25519 keypair and device ID. Manage paired devices from the
Mac UI: **Settings** → **Mobile Devices**.

## Connectivity Modes

Richter Mobile supports three connectivity modes:

### LAN Mode (Default)

The mobile app discovers the daemon via Bonjour (iOS) or NSD (Android) on
your local network. No internet required. Zero configuration.

- **Requires**: Mac and phone on the same local network.
- **iOS privacy**: The first time the app uses local network discovery,
  iOS shows a permission dialog. You must allow it.
- **Fallback**: If Bonjour/NSD discovery fails (some corporate networks,
  guest WiFi), enter the daemon's IP address and port manually in
  **Settings** → **Connectivity** → **Manual Entry**.

### VPN Mode

If your phone and Mac are on the same VPN (Tailscale, WireGuard, ZeroTier),
the daemon is reachable via its VPN IP address. Enter the VPN IP and port
in **Settings** → **Connectivity** → **Manual Entry**.

VPN mode is recommended for remote access if you already use a VPN — no
additional infrastructure needed.

### Relay Mode (Optional, Self-Hosted)

For remote access without a VPN, you can deploy a Richter Relay — a
lightweight WebSocket proxy that bridges your mobile app to your daemon.

- **Self-hosted**: You run the relay on your own infrastructure (VPS, home
  server, Raspberry Pi).
- **E2E encrypted**: The relay proxies encrypted WebSocket traffic. It
  cannot read your commands, logs, event data, or search queries.
- **Setup**: See [Remote Access](REMOTE_ACCESS.md) for the full relay
  setup guide.

For the full connectivity design, see **[ADR 0008](../adr/0008-local-first-mobile-remote-access.md)**.
For relay setup instructions, see **[Remote Access](REMOTE_ACCESS.md)**.

## Notifications

Richter Mobile can notify you of important events:

### Notification Types

| Event | Notified? | Channel | Importance |
|---|---|---|---|
| Agent action requires approval | ✅ Always | Approvals | Critical |
| Build failed | ✅ Default on | Builds | High |
| Test suite failed | ✅ Default on | Tests | High |
| Agent crashed or errored | ✅ Default on | Agents | High |
| Resource contention (CPU/memory) | ⚠️ Configurable | System | Medium |
| Build passed | ❌ Default off | Builds | Low (noise) |
| Test suite passed | ❌ Default off | Tests | Low (noise) |
| Agent started work | ❌ Never | Agents | None |

### Local vs Push Notifications

- **Local notifications**: When the app is in the foreground and connected
  to the daemon, events are shown as in-app banners via the WebSocket event
  stream. No push infrastructure needed.
- **Push notifications**: When the app is backgrounded or the phone is
  locked, push notifications are delivered via APNs (iOS) or FCM (Android).
  This requires configuring a push provider — see [Notifications](NOTIFICATIONS.md).

For the full notification design, see **[Notifications](NOTIFICATIONS.md)**.

## Transport Security Status

> **⚠️ Not yet implemented.** TLS for the mobile gateway is planned but not
> currently active. The `tls_enabled` config flag exists and defaults to `true`,
> but the TLS termination code is a stub — the gateway always listens on plain
> HTTP regardless of the flag. For production use, run a reverse proxy
> (nginx, Caddy) in front of the gateway to handle TLS termination until
> native TLS support lands. The pairing token and Ed25519 per-request signing
> provide authentication and integrity even without TLS, but traffic is not
> encrypted on the wire.

## Security and Privacy

Richter Mobile inherits Richter's local-first, cloud-optional security
philosophy. Key points:

- **No cloud database.** All data lives on your Mac's daemon. The mobile
  app caches data locally but the daemon is authoritative.
- **No accounts, no passwords.** Pairing uses QR codes with single-use
  tokens and Ed25519 device keypairs. No passwords to leak, phish, or
  brute-force.
- **Certificate pinning.** The mobile app pins the daemon's TLS certificate
  fingerprint from the pairing QR code. No man-in-the-middle, even on
  hostile networks.
- **Per-request signing.** Every API request is signed with the device's
  Ed25519 private key, stored in the hardware-backed secure enclave
  (Secure Enclave on iOS, TEE-backed Keystore on Android).
- **Biometric-gated approvals.** Approving or denying agent actions requires
  biometric confirmation (Face ID, Touch ID, or fingerprint).
- **Scope enforcement.** Each device has a capability set that limits what
  it can do. You can, for example, pair a device that can view runs but
  cannot approve actions.
- **No telemetry.** The mobile app sends no analytics or usage data to any
  third party unless you explicitly opt in.
- **E2E encrypted relay.** If you use the optional relay for remote access,
  all traffic is E2E encrypted. The relay cannot read any application data.

For the complete security model, see **[Mobile Security](MOBILE_SECURITY.md)**.

## Troubleshooting

### Pairing Troubleshooting

**QR code won't scan**

- Make sure the QR code is displayed at a reasonable size on your Mac
  screen. Reduce screen brightness if there's glare.
- Try the "Enter Pairing Code Manually" option below the QR code.
- If your phone camera is damaged, use manual code entry.

**"Pairing token expired"**

- The pairing token is valid for 120 seconds. Start a new pairing session
  from the Mac UI and scan the QR code immediately.

**"Device already paired"**

- This device's identity is already registered. If this is the same phone
  you paired before, it should connect automatically. Check
  **Settings** → **This Device** to see if a pairing exists.
- If this is a restored or reset phone, revoke the old device from the
  Mac UI and pair again.

**"Connection refused"**

- Verify the daemon is running on your Mac (`richter status` in Terminal).
- Verify your phone and Mac are on the same network.
- Check if a firewall on your Mac is blocking the daemon's port (default:
  19557). You may need to allow `richterd` in
  **System Settings** → **Network** → **Firewall**.

### Connection Troubleshooting

**"Disconnected" in the status bar**

- Verify your Mac is awake and on the same network.
- Go to **Settings** → **Connectivity** → **Scan for Daemon**.
- Try manual IP entry: find your Mac's IP address
  (**System Settings** → **Network** → **Wi-Fi** → **Details**), then enter
  it in the mobile app with port 19557 (e.g., `192.168.1.42:19557`).
- If using a relay, verify the relay is running and reachable.

**Local network permission denied (iOS)**

- Go to **Settings** → **Privacy & Security** → **Local Network** and
  enable Richter Mobile.
- If Richter Mobile is not listed, reinstall the app and accept the
  permission dialog when it appears.

**"Certificate mismatch" error**

- The daemon's TLS certificate has changed since you paired. This can happen
  if you manually replaced the certificate or if the daemon was reinstalled.
- Revoke the device from the Mac UI and re-pair.

### Notification Troubleshooting

**Not receiving push notifications**

- Push notifications require explicit setup. See [Notifications](NOTIFICATIONS.md).
- Verify notification permissions: iOS **Settings** → **Richter Mobile** →
  **Notifications**; Android **Settings** → **Apps** → **Richter Mobile** →
  **Notifications**.
- Check notification importance thresholds in the app:
  **Settings** → **Notifications**. If you've set the threshold to
  "Critical Only," most events will be suppressed.

## CLI Commands

The following `richter` CLI commands are relevant to mobile device
management:

```bash
# List paired mobile devices
richter mobile list

# Show details for a specific device
richter mobile show <device-id>

# Revoke a device
richter mobile revoke <device-id>

# Update device capabilities
richter mobile update <device-id> --capabilities read:status,write:approve

# Generate a new pairing QR code (prints to terminal; add --display for GUI)
richter mobile pair --display

# Show mobile gateway status
richter mobile status

# Test relay connection
richter mobile relay-test --relay wss://relay.example.com

# Rotate daemon keypair (requires re-pairing all devices)
richter mobile rotate-daemon-key
```

## Further Reading

- **[ADR 0005: Mobile Companion Architecture](../adr/0005-mobile-companion-architecture.md)** — Full architecture decision.
- **[ADR 0006: Mobile Pairing and Trust Model](../adr/0006-mobile-pairing-and-trust-model.md)** — Trust model, pairing ceremony, authentication.
- **[ADR 0007: Why React Native + Expo](../adr/0007-why-react-native-expo.md)** — Framework selection analysis.
- **[ADR 0008: Local-First Mobile Remote Access](../adr/0008-local-first-mobile-remote-access.md)** — Connectivity modes, relay design.
- **[Mobile Security](MOBILE_SECURITY.md)** — Complete security model and threat analysis.
- **[Remote Access](REMOTE_ACCESS.md)** — Relay setup and remote access guide.
- **[Notifications](NOTIFICATIONS.md)** — Notification design and configuration.
- **[Architecture](ARCHITECTURE.md)** — Overall Richter architecture.
- **[Security](SECURITY.md)** — Mac-side security model.
- **[User Guide](USER_GUIDE.md)** — Full Richter user guide.
