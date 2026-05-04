# ADR 0007: Why React Native + Expo for the Mobile Companion App

**Status**: Accepted
**Date**: 2026-05-04
**Author**: Alberto Nunez
**Deciders**: Alberto Nunez

## Context

Richter Mobile is a companion app that lets users monitor and approve AI
coding agent activity from their phone. It must run on both iOS and Android
and support: local network discovery (Bonjour/NSD), secure storage (Keychain/
Keystore), biometric authentication, push notifications, real-time event
streams (WebSocket/SSE), and a polished native-feeling UI.

We need to choose a mobile development framework.

## Decision

Richter Mobile is built with **React Native** using **Expo development
builds** (not Expo Go).

### The Short Version

React Native + Expo gives us iOS and Android from one TypeScript codebase
without sacrificing any of our hard requirements: Bonjour/NSD discovery,
secure enclave-backed key storage, biometric auth, push notifications, and
real-time WebSocket/SSE support. Expo development builds (as opposed to
Expo Go) provide full native module access, so we can use any React Native
library without ejecting from the Expo managed workflow. EAS Build handles
CI/CD for both platforms.

### Requirements Checklist

| Requirement | React Native + Expo | SwiftUI + Kotlin/Compose | Flutter | KMP | PWA |
|---|---|---|---|---|---|
| **iOS + Android from one codebase** | ✅ TypeScript | ❌ Two codebases | ✅ Dart | ⚠️ Shared logic only | ✅ Web |
| **Bonjour/NSD discovery** | ✅ react-native-zeroconf | ✅ | ⚠️ Limited plugins | ✅ | ❌ No API |
| **Secure enclave key storage** | ✅ expo-secure-store | ✅ | ✅ flutter_secure_storage | ✅ | ❌ Web Crypto only |
| **Biometric auth** | ✅ expo-local-authentication | ✅ | ✅ local_auth | ✅ | ⚠️ WebAuthn only |
| **Push notifications** | ✅ expo-notifications | ✅ | ✅ | ✅ | ⚠️ Limited on iOS |
| **WebSocket / SSE** | ✅ Built-in JS | ✅ | ✅ | ✅ | ✅ |
| **OTA updates** | ✅ EAS Update | ❌ App Store only | ✅ Shorebird | ❌ | ✅ |
| **Native-feeling UI** | ✅ With care | ✅ Best possible | ⚠️ Cupertino/Material separate | ⚠️ Needs native UI layer | ⚠️ Not truly native |
| **Ecosystem maturity** | ✅ Very mature | ✅ Mature | ⚠️ Growing | ⚠️ Early | N/A |
| **Team velocity** | ✅ One team, TS | ⚠️ Two teams (Swift + Kotlin) | ⚠️ Dart learning curve | ⚠️ Requires native UI teams | ✅ |
| **Accessibility** | ✅ | ✅ | ⚠️ Improving | ✅ | ✅ |
| **CI/CD simplicity** | ✅ EAS Build | ⚠️ Two pipelines | ✅ Codemagic | ⚠️ Complex | ✅ |
| **Background execution** | ✅ Via native modules | ✅ | ✅ | ✅ | ❌ Very limited |

### Why Not SwiftUI + Kotlin/Compose (Fully Native)

Building fully native would produce the highest polish per platform — SwiftUI
on iOS and Jetpack Compose on Android. This is what we do on macOS (SwiftUI
app alongside the Rust daemon).

**Rejected because**:

- **Two codebases, two languages, two teams** — Richter currently has a small
  team. Doubling the surface area for a companion app (not the primary
  surface) is not justifiable. Every feature, every UI tweak, every bug fix
  must be implemented twice.
- **Feature drift** — Native iOS and Android apps built by separate teams (or
  the same team context-switching) inevitably drift. The iOS app gets a new
  feature; the Android app lags behind.
- **Diminishing returns** — For a companion monitoring/approval app (not a
  full orchestrator), the native polish advantage is marginal. The app is
  primarily lists, status cards, and approval buttons. React Native handles
  these patterns extremely well.
- **Precedent** — Discord, Shopify, Microsoft (Office, Teams, Outlook),
  Meta (Facebook, Instagram), and Coinbase all use React Native for
  similarly complex apps. Native-only for a companion app is over-engineering.

If Richter Mobile ever becomes the primary orchestrator surface (contrary to
current architecture), we would revisit this decision.

### Why Not Flutter

Flutter provides excellent cross-platform UI from a single Dart codebase.
Its widget-based rendering is consistent across platforms and the hot-reload
developer experience is excellent.

**Rejected because**:

- **Dart ecosystem gap** — Richter's entire stack is Rust + TypeScript
  (SwiftUI on macOS). Adding Dart means adding a new language, new tooling,
  new package ecosystem, and new hiring requirements. TypeScript is already
  used throughout (the Mac app can share types via a shared types package).
- **Bonjour/NSD support** — Flutter's plugin ecosystem for local network
  discovery is immature compared to React Native's `react-native-zeroconf`
  (which wraps Apple's NSNetService and Android's NsdManager directly).
- **Native look-and-feel** — Flutter renders its own widgets. While
  Cupertino and Material widgets exist, they are approximations, not true
  native components. On iOS, subtle interaction differences (scroll physics,
  text selection, context menus) accumulate into an experience that feels
  slightly off. For a companion app that users interact with briefly but
  frequently, these rough edges matter.
- **Background execution** — Flutter's background execution model is less
  mature than React Native's native module approach for tasks like
  maintaining a WebSocket connection while the app is backgrounded.

### Why Not Kotlin Multiplatform (KMP)

KMP shares business logic across platforms while using native UI (SwiftUI on
iOS, Jetpack Compose on Android).

**Rejected because**:

- **UI still per-platform** — KMP only shares the non-UI layer. We'd still
  need to write SwiftUI for iOS and Compose for Android. This is better than
  fully native (shared logic reduces duplication) but still doubles UI work.
- **Ecosystem maturity** — KMP is promising but still early. Library
  availability, tooling stability, and community size are significantly
  smaller than React Native's.
- **No advantage over React Native for our use case** — KMP's strength is
  sharing complex business logic across platforms when UI is platform-native.
  Richter Mobile's business logic is thin: it's a view layer over the
  daemon's API. The value is in the UI surface, and React Native shares both
  logic and UI.

### Why Not PWA (Progressive Web App)

A PWA would be the simplest option: no App Store, no platform-specific code,
instant updates.

**Rejected because**:

- **No Bonjour/NSD** — Web browsers cannot discover local network services.
  The PWA would need to connect via a known IP:port, defeating the zero-config
  LAN discovery that is a core requirement.
- **No secure enclave** — Web Crypto API provides cryptographic operations
  but cannot store keys in the hardware-backed Keychain/Keystore. Ed25519
  private keys would be stored in IndexedDB, which is far less secure.
- **No background WebSocket** — iOS Safari aggressively suspends background
  web pages. A PWA cannot maintain a WebSocket connection for real-time
  event streaming when the phone is locked.
- **Push notifications limited on iOS** — iOS Safari push notification
  support is limited and requires the user to add the PWA to their home
  screen and grant notification permissions separately. The experience is
  not on par with native push.
- **No biometric auth integration** — WebAuthn provides some biometric
  capabilities but cannot gate in-app actions (approve/deny) with biometric
  confirmation. PWA would need a separate confirmation mechanism.
- **App Store absence** — While some users prefer PWAs, the App Store/Google
  Play Store provide trust signals, automatic updates, and discoverability
  that PWAs cannot match.

### Why Expo Development Builds (Not Expo Go)

Expo Go is the quickest way to start a React Native project — scan a QR code
and the app runs. But Expo Go ships with a fixed set of native modules.

**We use Expo development builds because**:

- **react-native-zeroconf** requires native code (wrapping NSNetService and
  NsdManager). It is not available in Expo Go.
- **expo-secure-store** is available in Expo Go, but custom Keychain access
  groups (for cross-app sharing or specific access policies) require a
  development build.
- **Background WebSocket** requires native headless task registration,
  which needs custom native code.
- **Ed25519 signing** via native crypto (for performance and secure enclave
  integration) requires native module bridging.

Expo development builds give us the managed workflow benefits (EAS Build,
OTA updates via EAS Update, config plugins) with full native module access.
We do not eject — we stay in the Expo managed workflow with config plugins
to handle native dependencies.

### Key Expo Libraries Used

| Library | Purpose |
|---|---|
| `expo-router` (or `@react-navigation/native`) | File-based or stack navigation |
| `expo-secure-store` | Keychain/Keystore storage for daemon info, cert fingerprints |
| `expo-local-authentication` | Biometric confirmation for approve/deny/cancel actions |
| `expo-notifications` | Push notification handling |
| `expo-crypto` | Cryptographic primitives (if needed; Ed25519 via native bridge) |
| `expo-background-fetch` | Periodic background sync with daemon |
| `expo-task-manager` | Background task registration |
| `react-native-zeroconf` | Bonjour/NSD daemon discovery |
| `react-native-mmkv` | High-performance local cache for event/run data |

### Project Structure

```
mobile/
├── app/                        # expo-router file-based routes
│   ├── _layout.tsx             # Root layout (providers, theme)
│   ├── index.tsx               # Now view (home)
│   ├── repos.tsx               # Repo list
│   ├── runs/
│   │   ├── index.tsx           # Run history list
│   │   └── [id].tsx            # Single run detail
│   ├── approvals.tsx           # Pending approvals
│   ├── search.tsx              # Search
│   └── settings/
│       ├── index.tsx           # Settings root
│       ├── this-device.tsx     # Device info, revoke, rotate key
│       ├── connectivity.tsx    # Connectivity mode
│       ├── notifications.tsx   # Notification prefs
│       └── about.tsx           # About, licenses
├── src/
│   ├── api/                    # Daemon API client
│   │   ├── client.ts           # HTTP + WebSocket client
│   │   ├── auth.ts             # Request signing, pairing
│   │   └── discovery.ts        # Bonjour/NSD discovery
│   ├── components/             # Shared UI components
│   │   ├── StatusCard.tsx
│   │   ├── RunListItem.tsx
│   │   ├── ApprovalCard.tsx
│   │   └── ...
│   ├── hooks/                  # Custom hooks
│   │   ├── useDaemon.ts        # Daemon connection hook
│   │   ├── useWebSocket.ts     # Real-time event stream
│   │   └── ...
│   ├── stores/                 # State management (zustand)
│   │   ├── daemonStore.ts
│   │   ├── runStore.ts
│   │   └── ...
│   ├── crypto/                 # Ed25519, signing, verification
│   │   ├── keys.ts
│   │   └── sign.ts
│   ├── storage/                # Keychain/Keystore wrapper
│   │   └── secureStorage.ts
│   └── types/                  # Shared TypeScript types
│       ├── daemon.ts
│       ├── events.ts
│       └── ...
├── app.json                    # Expo config
├── eas.json                    # EAS Build config
├── tsconfig.json
└── package.json
```

### Technology References

- [React Native documentation](https://reactnative.dev/docs/getting-started)
- [Expo development builds](https://docs.expo.dev/develop/development-builds/introduction/)
- [Expo development builds vs Expo Go](https://docs.expo.dev/develop/development-builds/use-development-builds/)
- [EAS Build](https://docs.expo.dev/build/introduction/)
- [EAS Update](https://docs.expo.dev/eas-update/introduction/)
- [Expo config plugins](https://docs.expo.dev/config-plugins/introduction/)
- [react-native-zeroconf](https://github.com/balthazar/react-native-zeroconf)
- [expo-secure-store](https://docs.expo.dev/versions/latest/sdk/securestore/)
- [expo-local-authentication](https://docs.expo.dev/versions/latest/sdk/local-authentication/)
- [expo-notifications](https://docs.expo.dev/versions/latest/sdk/notifications/)
- [expo-crypto](https://docs.expo.dev/versions/latest/sdk/crypto/)
- [Flutter documentation](https://flutter.dev/docs)
- [Kotlin Multiplatform](https://kotlinlang.org/docs/multiplatform.html)
- [PWA on MDN](https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps)

## Consequences

### Positive

- One TypeScript codebase for iOS and Android maximizes team velocity and
  ensures feature parity.
- Expo development builds give full native module access without ejecting
  from the managed workflow.
- EAS Build provides CI/CD for both platforms without maintaining native
  build infrastructure.
- EAS Update enables OTA JavaScript updates, bypassing App Store review
  for non-native changes.
- TypeScript shared types between the Mac and mobile codebases reduce
  integration drift.
- react-native-zeroconf provides battle-tested Bonjour/NSD support.
- Large ecosystem of high-quality libraries for secure storage, biometrics,
  notifications, and networking.

### Negative

- React Native adds a JavaScript runtime and bridge overhead vs. fully
  native apps. For a companion app with moderate UI complexity, this is
  negligible.
- The JavaScript-to-native bridge can become a bottleneck for complex
  animations or heavy computation. Not a concern for our list-and-card UI.
- React Native version upgrades can be painful (though Expo's managed
  workflow and upgrade helper mitigate this significantly).
- Debugging native module issues requires some platform-specific knowledge.
- Expo's SDK release cadence (approximately quarterly) means a slight lag
  behind the latest React Native releases.
- The app cannot be distributed as a PWA, which eliminates users who prefer
  not to install apps from app stores.

### Mitigations

- Use Expo's upgrade helper and `npx expo-doctor` for version upgrades.
- Keep native module usage minimal and well-documented — only Bonjour/NSD,
  secure storage, and biometrics.
- Use EAS Update for rapid iteration; App Store review only needed for
  native dependency changes.
- Expose TypeScript types from a shared package (`@richter/types`) that
  both the mobile app and the daemon's API documentation consume.
- Use React Native's New Architecture (Fabric renderer + TurboModules) when
  it reaches Stable in Expo to minimize bridge overhead.
