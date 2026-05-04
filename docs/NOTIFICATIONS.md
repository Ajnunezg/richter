# Richter Mobile Notifications

## Notification Philosophy

Richter Mobile's notification philosophy is: **quieter than macOS, more
urgent than email.**

The Mac app shows everything — every run, every event, every agent status
change. It's your full dashboard. The mobile app is not a second dashboard.
It's a **signal channel** for things that need your attention when you're
away from your desk.

Principles:

1. **Notify on attention-needed, not on information-available.** A build
   passing is information. A build failing is attention-needed.
2. **Default to quiet.** Too many notifications train users to ignore all
   notifications. We'd rather miss a low-importance event than cause
   notification fatigue.
3. **Rich on tap, generic in payload.** Push notification payloads contain
   only metadata. Full content is fetched when the user opens the app.
4. **Configurable thresholds.** Every user has a different tolerance for
   interruptions. Let them tune it.
5. **Respect platform conventions.** iOS notification categories and
   Android notification channels behave as users expect.

## What Gets Notified vs. What Doesn't

### Always Notified (Cannot Be Disabled)

| Event | Category | Priority | Rationale |
|---|---|---|---|
| Agent action requires your approval | Approvals | Critical | If you don't approve/deny, work stalls or an agent proceeds without oversight |
| Agent crash or unrecoverable error | Agents | High | An agent is down and work has stopped |
| Pairing request from new device | Security | Critical | Someone might be trying to pair a malicious device |

### Default On (Can Be Disabled)

| Event | Category | Priority | Rationale |
|---|---|---|---|
| Build failure | Builds | High | A build broke; you probably want to know |
| Test suite failure | Tests | High | Tests are failing |
| Lint failure | Tests | Default | Code quality regression |
| Agent stuck (no progress for 5+ minutes) | Agents | Default | Agent might be in a loop |
| Resource exhaustion (CPU > 90% for 5+ min) | System | Default | Your Mac is struggling |

### Default Off (Can Be Enabled)

| Event | Category | Priority | Rationale |
|---|---|---|---|
| Build passed | Builds | Low | Positive signal, but usually noise |
| Test suite passed | Tests | Low | Positive signal, but usually noise |
| Agent started a new run | Agents | Low | Informational; not actionable |
| Agent completed a run | Agents | Low | Informational; not actionable |
| Resource contention (two builds competing) | System | Low | Interesting but usually self-resolving |

### Never Notified

| Event | Rationale |
|---|---|
| Agent heartbeat / keepalive | Infrastructure noise |
| File watcher events (file changed, git branch switch) | High volume, low signal |
| MCP tool invocations (individual tool calls) | Far too granular |
| Cache hits, lease acquisitions | Internal mechanics |
| Telemetry / analytics events | Internal only |

## Notification Categories and Channels

### iOS Notification Categories

| Category Identifier | Display Name | Actions | Importance |
|---|---|---|---|
| `richter.approval` | Agent Approvals | Approve, Deny, View | Critical (time-sensitive) |
| `richter.build` | Build Results | View Run, Mute Repo | High (default) |
| `richter.test` | Test Results | View Run, Mute Repo | High (default) |
| `richter.agent` | Agent Status | View Agent, Cancel Agent | Default |
| `richter.system` | System Alerts | View Details | Default |
| `richter.security` | Security | View Device, Revoke | Critical (time-sensitive) |

### Android Notification Channels

| Channel ID | Display Name | Importance | Vibration | Sound |
|---|---|---|---|---|
| `richter_approvals` | Agent Approvals | HIGH (pops on screen) | Yes | Yes |
| `richter_security` | Security Alerts | HIGH (pops on screen) | Yes | Yes |
| `richter_builds` | Build Results | DEFAULT (makes sound) | Yes | Yes |
| `richter_tests` | Test Results | DEFAULT (makes sound) | Yes | Yes |
| `richter_agents` | Agent Status | DEFAULT (makes sound) | Optional | Optional |
| `richter_system` | System Alerts | LOW (no sound) | No | No |

## Local / In-App Notifications

When the app is **in the foreground** and connected to the daemon (via
WebSocket), events are delivered as **in-app banners**. These are not OS
notifications — they appear as a subtle banner at the top of the app
that auto-dismisses after 4 seconds.

In-app banners follow the same importance thresholds as push notifications.
Users can tap the banner to jump to the relevant run detail, or swipe it
away to dismiss.

In-app banners do not require push infrastructure — they work entirely
over the WebSocket event stream. This is the primary notification
mechanism when you're actively using the app.

## Push Notifications

Push notifications deliver events when the app is **backgrounded** or the
phone is **locked**. They use Apple Push Notification service (APNs) on iOS
and Firebase Cloud Messaging (FCM) on Android.

### Push Is Opt-In

Push notifications require explicit setup: a push provider must be
configured in the daemon, the mobile app must request notification
permissions, and the user must not have disabled them in system settings.

Without push configuration, the app relies on in-app banners when
foregrounded, and periodic background fetch as a best-effort fallback.

### Generic / Redacted Push Payloads

Push notification payloads contain only metadata — never application content:

```json
{
  "aps": {
    "alert": {
      "title": "Build failed",
      "body": "myproject:main — 3 tests failed, 42 passed"
    },
    "category": "richter.build",
    "thread-id": "repo_myproject",
    "sound": "default",
    "badge": 3
  },
  "richter": {
    "event_type": "build_failed",
    "run_id": "run_01JQ3XYZ...",
    "repo": "myproject",
    "importance": "high"
  }
}
```

Notice what's **not** in the payload: no command output, no log content,
no file paths, no error stack traces, no secrets, no full repo paths,
no agent reasoning traces.

When the user taps the notification, the app opens and fetches the full
event content from the daemon (via LAN or relay).

### Push Provider Integration

The daemon sends push notifications via the configured provider.

**APNs Provider (iOS)**:

```toml
[mobile.push]
provider = "apns"
apns_key_id = "ABC1234567"
apns_team_id = "DEF7890123"
apns_key_path = "/Users/alberto/.richter/certs/apns_auth_key.p8"
apns_topic = "com.richter.mobile"
apns_environment = "production"
```

**FCM Provider (Android + iOS Fallback)**:

```toml
[mobile.push]
provider = "fcm"
fcm_service_account_path = "/Users/alberto/.richter/certs/fcm_service_account.json"
fcm_project_id = "richter-mobile"
```

### Push Token Registration

1. App requests a push token from APNs (iOS) or FCM (Android).
2. App sends the push token to the daemon: `POST /mobile/push-token` with
   the signed Ed25519 request.
3. Daemon stores the push token associated with the device ID.

Push tokens are refreshed on reinstall, backup restore, or OS token
rotation. The mobile app re-registers its push token on each launch.

## Expo Notification Library Usage

Richter Mobile uses `expo-notifications` for cross-platform notification
handling.

Key workflows:

- **Request permissions**: iOS `.requestPermissionsAsync()` with alert,
  badge, sound, and critical alert support for approvals.
- **Push token**: `.getExpoPushTokenAsync()` and send to daemon.
- **Foreground handling**: `.setNotificationHandler()` with importance
  threshold filtering.
- **Notification tap**: `.addNotificationResponseReceivedListener()` to
  navigate to the relevant screen.
- **Notification categories**: `.setNotificationCategoryAsync()` for iOS
  approve/deny actions with `authenticationRequired: true`.
- **Background actions**: `TaskManager.defineTask('NOTIFICATION_ACTION')`
  for lock-screen approve/deny without opening the app.

## Configuring Importance Thresholds

Users can configure which events trigger notifications in
**Settings → Notifications → Importance Threshold**:

| Setting | Behavior |
|---|---|
| **All Events** | Every event, including passes and starts. Noisy. |
| **Failures + Warnings** | Build/test failures, agent errors, resource warnings. Default. |
| **Failures Only** | Only build/test failures and agent errors. |
| **Critical Only** | Only approvals, agent crashes, and security alerts. |
| **Approvals Only** | Only agent approval requests. Most quiet. |
| **None** | No notifications except pairing requests. |

These thresholds apply to both push notifications and in-app banners.
Per-repo overrides allow muting a noisy repo while keeping notifications
for others.

### Per-Repo Muting

From a run notification: mute for 1 hour, 4 hours, until tomorrow, until
next build passes, or permanently (until unmuted in Settings).

### Quiet Hours

**Settings → Notifications → Quiet Hours**: suppress notifications during
specified hours. Critical events (approvals, security) always break through.

## Notification Grouping and Badging

- **iOS grouping**: By thread ID: `repo_<name>`, `agent_<name>`, `approvals`,
  `security`.
- **Android grouping**: By group key with summary notifications for 3+
  notifications in a group.
- **Badge**: Shows count of unread approval requests, build/test failures,
  and agent errors. Informational events do not increment badge.

## Testing Notifications

```bash
# Test push via Expo API
curl -H "Content-Type: application/json" \
  -X POST "https://exp.host/--/api/v2/push/send" \
  -d '{"to":"<ExpoPushToken>","title":"Build failed","body":"myproject:main","data":{"richter":{"event_type":"build_failed","run_id":"test_123","repo":"myproject","importance":"high"}},"categoryId":"richter.build"}'

# Test via daemon
richter mobile notify-test --device <device-id> --type build_failed

# Check push delivery status
richter mobile push-status --device <device-id>
```

Test notification thresholds by changing settings and verifying only the
expected events arrive. Test notification actions by triggering an approval
request and using the lock-screen Approve/Deny actions.

## Batching and Rate Limiting

To avoid notification storms, the daemon batches notifications:

- **Batch window**: 30 seconds. Notifications for the same repo are batched.
- **Batch format**: "12 tests failed, 230 passed" rather than 12 separate
  notifications.
- **Max per minute**: 10 distinct deliveries per device per minute.
- **Critical override**: Approval and security notifications are never
  batched and never dropped.

## Daemon Configuration Reference

```toml
[mobile.push]
provider = "apns"  # or "fcm" or "both"
apns_key_id = "ABC1234567"
apns_team_id = "DEF7890123"
apns_key_path = "/Users/alberto/.richter/certs/apns_auth_key.p8"
apns_topic = "com.richter.mobile"
apns_environment = "production"
fcm_service_account_path = "/Users/alberto/.richter/certs/fcm_service_account.json"
fcm_project_id = "richter-mobile"

[mobile.push.batching]
max_notifications_per_minute = 10
batch_window_seconds = 30
max_batch_size = 5
```

## Privacy Guarantees

- No event content sent to Apple (APNs) or Google (FCM) beyond generic
  push payloads.
- Full event data fetched from daemon over encrypted channel after user
  opens notification.
- Push tokens stored only on daemon and push provider — not in analytics
  or telemetry.
- Disabling notifications removes push tokens from daemon and provider.
- The relay (if used) never sees push notification payloads.
- Notification preferences and mute lists are stored on the daemon.

## Further Reading

- **[ADR 0008: Local-First Mobile Remote Access](../adr/0008-local-first-mobile-remote-access.md)** — Push notification privacy.
- **[Mobile](MOBILE.md)** — Full mobile documentation.
- **[Mobile Security](MOBILE_SECURITY.md)** — Notification security.
- **[expo-notifications documentation](https://docs.expo.dev/versions/latest/sdk/notifications/)**
- **[Apple Push Notification service](https://developer.apple.com/documentation/usernotifications)**
- **[Firebase Cloud Messaging](https://firebase.google.com/docs/cloud-messaging)**
