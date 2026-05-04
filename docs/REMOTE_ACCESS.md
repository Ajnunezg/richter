# Richter Remote Access

Richter Mobile defaults to local-network-only connectivity. Remote access over the internet is an **opt-in, privacy-preserving extension** for users who want to monitor their Mac while away from home and do not want to configure a VPN.

## Principle

Remote access exists because LAN-only is insufficient for many real-world development workflows — but it must not compromise the core Richter thesis: local-first, no telemetry by default, and no cloud dependency.

## Three Connectivity Modes

### Mode A: Local LAN (Default)

The mobile app discovers the Mac on the same Wi-Fi network using Bonjour (iOS) or Network Service Discovery (Android). All traffic stays on the local network.

```bash
# Enable LAN mode
richter mobile enable --lan
```

### Mode B: Manual / VPN

For users on Tailscale, WireGuard, ZeroTier, corporate VPNs, or manually configured networks:

- Enter the Mac's IP address and port manually
- Use QR pairing to exchange credentials
- No Bonjour discovery required
- Works over any routable IP (VPN or direct)

### Mode C: Optional Remote Relay

For users who want phone access without configuring a VPN. This is the **most sensitive mode** and requires explicit configuration.

```bash
# Enable relay
richter mobile relay enable

# Check relay status
richter mobile relay status
```

## Relay Architecture

```
┌──────────┐         ┌──────────┐         ┌──────────┐
│  Mac     │◄───────►│  Relay   │◄───────►│  Phone   │
│ (daemon) │  E2E    │ (server) │  E2E    │  (app)   │
└──────────┘         └──────────┘         └──────────┘
```

### What the relay sees

The relay operates on **metadata only**:
- Channel ID (opaque)
- Connection timestamps
- Byte counts
- IP addresses (necessary for operation)

### What the relay CANNOT see

All payload content is end-to-end encrypted between the Mac and the phone:
- Repo names
- Commands
- Log content
- Event summaries
- Agent names
- File paths
- Approval details
- Notification content

### Self-hosted relay

The self-hosted relay is the first and recommended relay mode. Provide a small Rust binary:

```bash
# Build the relay server
cargo build --release -p richter-relay

# Run on a VPS or home server
./richter-relay --bind 0.0.0.0:8443 --tls-cert cert.pem --tls-key key.pem
```

The user deploys it to:
- A VPS (DigitalOcean, Hetzner, Linode, AWS)
- A home server or NAS
- Fly.io, Render, Railway
- Any Cloudflare Tunnel-compatible environment

### Managed relay

A first-party managed relay may be offered in the future. It will follow the same privacy guarantees:
- Opt-in only
- No telemetry by default
- No raw content visibility
- E2E encrypted
- Revocable
- Documented
- Priced/limited separately if needed

## Push Notifications with Remote Access

Push notifications require a push provider (APNs for iOS, FCM for Android, or Expo Push). Because push providers introduce third-party infrastructure:

- Push notifications are **opt-in**
- Push payloads are **generic by default** — no raw command text
- Event content is fetched from the daemon or relay **after the user opens the app**

### Safe push payload

```json
{
  "type": "important_event",
  "event_id": "evt_...",
  "importance": 92,
  "category": "run_failed"
}
```

The mobile app fetches the actual event content from the paired daemon or relay when the user opens the notification.

## iOS Background Constraints

iOS suspends apps aggressively in the background. Richter Mobile cannot guarantee an always-open WebSocket.

Strategies:
- **Foreground**: live WebSocket event stream
- **Background**: short reconnect window (~30 seconds via `beginBackgroundTask`)
- **Wakeup**: push notification for high-importance events
- **No battery drain**: no polling loop, no fake "always live" guarantee

## Android Background Constraints

Android offers more flexibility than iOS but still restricts background network access.

Strategies:
- **Foreground**: live WebSocket event stream
- **Background**: WorkManager for periodic sync (minimum 15-minute intervals)
- **Wakeup**: FCM high-priority push for critical events
- No foreground service unless a user-visible operation needs it

## Security Guarantees

- All relay traffic is E2E encrypted (TLS from daemon → relay, relay → phone, with additional payload encryption)
- Device-to-daemon authentication persists through the relay
- Relay cannot impersonate the daemon or the phone
- Relay cannot modify or inject events
- Metadata minimization: channel ID, timestamps, byte counts only
- Audit log on the daemon records all relay sessions

## Setting Up Remote Access

### 1. Deploy the relay server

```bash
# On your VPS or home server
git clone https://github.com/ajnunezg/richter.git
cd richter
cargo build --release -p richter-relay
./target/release/richter-relay --bind 0.0.0.0:8443 --tls-cert /path/to/cert.pem --tls-key /path/to/key.pem
```

### 2. Configure the daemon

```bash
# On your Mac
richter mobile relay enable --host relay.example.com --port 8443
richter mobile relay status
```

### 3. Configure the mobile app

Settings → Relay → Enter host and port → Connect

### 4. Verify

```bash
richter doctor --mobile
```

## Firewall Considerations

When using LAN mode:
- The Mac firewall must allow inbound connections on the Mobile Gateway port
- The phone and Mac must be on the same subnet for Bonjour/NSD discovery
- Check: System Settings → Network → Firewall → Options

When using relay mode:
- The Mac needs outbound internet access to the relay server
- No inbound ports need to be opened on the Mac
- No router configuration needed

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Phone can't discover Mac | Different subnet/VLAN | Use manual IP entry |
| "Connection refused" | Firewall blocking | Allow richter-daemon in firewall |
| Relay won't connect | TLS certificate issue | Verify cert paths and validity |
| Push notifications not arriving | Push provider not configured | Check push token registration |
| "Device revoked" after reinstall | App reinstall invalidates credentials | Re-pair the device |
