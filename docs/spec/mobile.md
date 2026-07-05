# Mobile Client Specification (M7)

The native phone client: a Tauri 2 mobile shell around the same mobile web client that ships as the M6 LAN PWA, speaking the same authenticated WS protocol (`protocol.md`) to `dflowd`.
This spec replaces the deferred stub in `roadmap.md` (decision recorded 2026-07-04) and re-verifies the stub's claims against current platform reality instead of inheriting them.
The phone never bundles the daemon; it is a pure client at every tier, like every other client.

## 0. The stub's claims, verified (research 2026-07-04)

The M7 stub made specific claims; each was checked against current sources (all citations in section 8) rather than inherited.

| Stub claim | Verdict |
|---|---|
| Tauri 2 mobile: iOS + Android first-class since 2.0 stable | Confirmed with caveats: supported targets since 2024-10-02 and steadily maintained (2.11.5 as of 2026-07-01), but the project's own release posts say mobile DX and plugin coverage trail desktop, and no later official post declares that gap closed. |
| The M6 web client carries over nearly whole | Confirmed; section 2.2 formalizes the package split that makes it true by construction. |
| Native buys "real push notifications (iOS PWAs remain second-class)" | Sharpened, in native's favor: the M6 PWA on a plain-HTTP LAN origin is not a secure context, so it gets no service worker and **no push at all, on either OS**; iOS PWA push additionally requires HTTPS plus manual home-screen install and has a documented record of silently dying subscriptions. But native push is not free either: Tauri has **no official push support** (tracking issue open, official PR stalled in draft since early 2025), and push needs an internet-reachable relay regardless. Section 4 is the honest treatment. |
| Native buys OS keychain storage for the device token | Confirmed as a native-only win (the web client has nothing better than browser storage), but there is **no official Tauri keychain plugin**; M7 ships a small in-repo one (section 2.3). |
| Native buys app-store or sideload distribution and app identity | Confirmed; official App Store and Google Play distribution guides exist, and a production Tauri app is verified on Google Play. No established Tauri 2 app could be verified on the iOS App Store; treated as a stated risk. |
| Mobile plugin coverage trails desktop; mobile dev experience younger | Confirmed, unevenly: Android practitioner reports are strongly positive; iOS reports center on Xcode/signing friction, and there is an open bug where a backgrounded webview resumes blank (tauri#14371). |
| iOS builds require macOS + Xcode + Apple developer account | Confirmed. |
| The phone never bundles the daemon | Held as invariant throughout this spec. |

Two research findings the stub did not anticipate:

- **The secure-context wall is the real PWA killer, and it is platform-neutral.** No service worker, no push, no camera (QR scanning), and no WebAuthn on `http://<LAN-IP>` origins, on both iOS and Android.
  The native app is the only client tier that can scan a pairing QR in-app or ever raise a notification.
  Feedback to M6: the PWA's pairing flow cannot use the phone camera from the page; it must rely on the OS camera opening the encoded URL in the browser, or manual entry.
- **iOS home-screen web apps are stable but not contractually committed.** Apple shipped Declarative Web Push (iOS 18.4) and made every added site open as a web app by default (iOS 26), which signals investment; but the February 2024 EU episode, where Apple disabled home-screen web apps and reversed only under pressure, is why the native app, not the PWA, is the long-term phone story.

## 1. Product scope

The phone is an attention surface, not a second cockpit.
It exists to answer one question - "what needs me right now?" - and to let the user resolve those items from wherever they are.
The desktop remains the only place work is shaped, dispatched, configured, and watched in depth.

In scope (mirrors the phone capability profile in `security.md`):

- **Needs You queue**: the ranked, cross-project queue, each item deep-linking to its resolving surface; this is the home screen of the app.
- **Approvals**: plan approval, gate findings that need judgment, and the merge decision on a green PR.
- **Plan-round review**: the Plan Studio artifact chrome, touch-tuned (section 2.4); annotate, answer embedded controls, send the batch, approve.
- **Steering**: send a message into a session via `session.send_verified`; the same verified-submit guarantees as desktop.
- **Read-only terminal peek**: styled screen snapshot plus recent scrollback for any session; look, do not type.
- **Session resume trigger**: tap an `interrupted` session to fire `session.resume`; the daemon does the actual resume in the worktree.
- **Fleet glance**: a compact Mission Control strip (sessions with lifecycle state chips, elapsed time, last status note).

The one-click invariant (`product.md`) translates to one tap: every representation of an agent or blocking item opens its resolving surface in exactly one tap, and notifications deep-link the same way (section 4.4).

Non-goals, explicit:

- No full interactive terminals; the capability profile grants read-only terminals and the UI does not render a keyboard path into a PTY.
- No vault access of any kind; secret-bearing streams (env endpoints, raw scrollback) are excluded from remote scopes entirely per `security.md`.
- No recipe editing or recipe install.
- No daemon on the phone, ever; no degraded "local mode".
- No project registration, repo browsing, or diff review; the Diff tab stays desktop-only in M7.
- No offline action queue: approving or merging against stale state is worse than waiting, so mutating actions are disabled while disconnected (section 3.3).

## 2. Architecture

### 2.1 A pure protocol client

The phone app is a client of `dflowd` and nothing else.

- It authenticates with `auth.hello { client: mobile }` and speaks the same message families as every other client; M7 adds no mobile-specific protocol surface, and any future needs evolve payloads additively per `protocol.md` versioning rules.
- The shell's Rust side runs zero DapperFlow engine code: no `dflow-core`, no store, no adapters.
  It exists only for the Tauri runtime, plugins, and OS integration.
- All protocol traffic originates in the webview (shared `client-core`), with one exception: the Android connected-notifications service (section 4.2) holds its own narrow event subscription outside the webview.

### 2.2 Shared client packages (the three-client split)

Today `apps/desktop` is a single package with `protocol.ts`, `client.ts`, `model.ts`, and state inline.
Before the M6 PWA ships, this extracts into a pnpm workspace so the desktop webview, the M6 PWA, and the M7 mobile app are three shells over one client codebase:

- **`packages/client-proto`**: TypeScript protocol types generated from the `dflow-proto` Rust crate (ts-rs or specta), so no client can drift from the daemon; the generated output is checked in and CI fails on staleness.
- **`packages/client-core`**: framework-agnostic transport and state: WS connect/auth/reconnect, event-cursor persistence and replay, entity caches (cards, sessions, fleet), Needs You derivation, binary PTY frame decode, verified-submit and artifact-feedback calls.
  It depends only on WebSocket, fetch, and an injected storage interface; no DOM, no React, no service worker.
- **`packages/client-ui`**: shared React components that survive form-factor changes: lifecycle state chips, session strips, timeline entries, Needs You items, and the artifact chrome host.
- **`packages/platform`**: the seam that makes M7 cheap.
  One interface for notifications, secure token storage, deep-link handling, QR scanning, and biometrics; three implementations: desktop (Tauri desktop APIs), web (PWA APIs with graceful degradation), mobile (Tauri mobile plugins).
  The interface includes a push-registration capability that today returns `unsupported` everywhere; section 4.5 explains why the slot exists anyway.
- **`apps/desktop`**, **`apps/web`** (M6), **`apps/mobile`** (M7): shells.
  `apps/mobile` renders the same mobile-tuned React app as `apps/web`; the differences are the platform implementation bound at build time and the shell packaging.

The invariant: a feature lands once in `client-core`/`client-ui` and all three clients get it; a shell may only add OS integration, never business logic.

### 2.3 Tauri 2 mobile shell

Verified maturity picture as of July 2026 (citations in section 8):

- Tauri 2.0 went stable 2024-10-02 with iOS and Android as supported targets; the project's own posts state that mobile DX and plugin coverage trail desktop, and no later official post declares that gap closed.
  The framework has advanced through steady 2.x minors (2.11.5 as of 2026-07-01) with incremental mobile fixes.
- Practitioner evidence splits by platform: a late-2025 retrospective of four working Android Tauri apps reports an excellent iteration loop (HMR on device, features prototyped in hours), while iOS feedback threads report friction dominated by Xcode, signing, and stale docs.
- Store-shipped existence proofs are real but thin: a production digital-signage client verified on Google Play ("built on a robust Rust and Tauri architecture"), plus working daily-use apps outside the stores; no established Tauri 2 app could be verified on the iOS App Store.
  This spec accepts that as a stated risk, mitigated by the fallback ladder (section 2.5) and by M7a shipping the thinnest possible shell first.

Per-platform facts that shape the plan:

- **iOS**: WKWebView; building requires macOS plus Xcode, and distribution requires an Apple Developer account.
  There is an open bug where a backgrounded webview can resume blank (tauri#14371), so M7a acceptance includes a background-resume soak test, and the shell implements full state restore on resume; that restore is cheap by design because client state is just the event cursor plus caches (section 3.3).
- **Android**: system WebView; builds run on any desktop OS, including the Windows dev machine; the Play pipeline is proven by shipped apps.

Official plugins M7 uses: `barcode-scanner` (in-app QR pairing), `deep-link` (custom scheme plus universal links / app links; schemes must be declared statically, no runtime registration), `notification` (local notifications only; see section 4), `biometric` (iOS + Android; `authenticate()` is pass/fail only and does not gate keychain entries).

Secure token storage has **no official plugin**: the official `stronghold` plugin is a password-derived encrypted file vault, not hardware-backed Keychain/Keystore.
Decision: M7 ships a small in-repo shell plugin (one Swift file, one Kotlin file) for Keychain/Keystore storage of the device token, following the published DecentPaste implementation guide, which also documents the sharp edges (biometric re-enrollment invalidating keys, async Android callbacks).
The community alternatives (impierce/tauri-plugin-keystore, Choochmeque/tauri-plugin-biometry) are alpha-stage single-maintainer projects, and token storage is too security-critical to outsource there; if one matures before M7a, adopting it is a welcome simplification, and the `packages/platform` seam makes the swap invisible.

### 2.4 Artifact review on the phone

Plan-round review uses the same architecture as desktop (`security.md`, `plan-studio.md`): a sandboxed `<iframe>` with an opaque origin, artifact documents served by the daemon's HTTP endpoint via short-lived signed URLs, the review SDK injected server-side, and postMessage with schema validation as the only channel.

What carries over unchanged:

- Signed URLs mean the iframe never holds a bearer token; on a pocketable device this matters even more.
- The quote-anchored annotation model is inherently touch-friendly: the load-bearing anchor is quoted text, not pixel offsets.
- Explore mode's pan and zoom become pinch and pan; annotate mode uses native touch text selection.
- The layout audit runs at the phone's viewport width and reports it in `layout_warnings`, so agents learn to author artifacts that survive 390 px without any new machinery.

Platform constraints found in research, and how M7 handles them:

- **`frame-ancestors` must allowlist the mobile shell origins.** The artifact CSP is `frame-ancestors <app-origin>`, and the mobile app's origin is the Tauri shell origin (custom scheme `tauri://localhost` on iOS, `http://tauri.localhost` on Android), not the desktop's.
  The daemon derives the allowed ancestor from the authenticated client type at URL-signing time.
  That custom-scheme origins break third-party `frame-ancestors` checks is on record (tauri#14056); ours is fixable because we control both ends.
- **Cross-scheme iframe loading is the biggest platform unknown.** A main frame on a custom scheme loading a plain-HTTP iframe plus subresources touches ATS, mixed-content, and secure-context behavior at once, and Tauri has a history of "resource requested insecurely" iframe issues around custom protocols (tauri#12767, tauri#3543).
  This is a **gating spike inside M7a**: prove artifact iframe, signed URLs, and the postMessage SDK on a real iPhone and a real Android device before building on top.
  Fallback if WKWebView blocks it: serve the entire review chrome from the daemon's HTTP origin (same origin as the artifact endpoint) in a dedicated in-app webview, eliminating the cross-scheme mix; the chrome is already a self-contained web surface, so this is packaging, not a rewrite.
- **Service workers are effectively unavailable inside WKWebView** without the app-bound-domains opt-in, so nothing in the shared client may require one; `client-core` does not by construction (2.2), and only the PWA shell uses a service worker (for installability, where its origin permits one at all).

### 2.5 Fallback ladder (re-affirmed)

The stub's ladder survives verification, with sharpened tripwires:

1. **Tauri 2 mobile** (this spec): default, because the M6 web client carries over nearly whole and the stack stays single-framework.
2. **Thin hand-rolled native shell** (Swift/WKWebView, Kotlin/WebView, wrapping the same web client): the fallback if the Tauri shell itself proves unstable, e.g. tauri#14371-class lifecycle bugs recurring without upstream fixes, or the M7a artifact spike failing in ways custom shell code could fix but Tauri cannot express.
   Everything in `packages/*` survives; only the shell layer is rewritten, and the notification-service design (4.2) transfers as-is.
3. **React Native / Flutter rewrite**: explicit last resort; it forks the client codebase, and nothing in current research suggests webview rendering itself is the problem.

## 3. Connectivity model

### 3.1 LAN-direct first

M7 connects the same way M6 does: to the daemon's opt-in LAN listener, over the same WS protocol, using a phone-scoped capability token.

- **Pairing** is the M6 QR flow: the desktop shows a QR encoding the LAN URL plus a pairing credential; scanning is the whole flow.
- The native app scans in-app via the barcode-scanner plugin as the primary path, so the credential never transits any other app.
- M7 formalizes pairing as **redemption**: the QR carries a short-lived single-use pairing token which the app immediately exchanges over the WS connection for a durable per-device capability token; the device token is what lands in the keychain and what per-device revocation targets.
  This is a strict tightening of the M6 flow, and the daemon-side pairing endpoint is shared with the PWA.
- A registered deep link (`dapperflow://pair?...`) also opens the app from an OS-camera scan as a convenience; because the QR credential is single-use and short-lived, transiting the system camera is acceptable.

Plain-HTTP-on-LAN is a deliberate, platform-visible choice (per `security.md`, LAN v1 ships without TLS), and the native shells must declare it:

- iOS: an App Transport Security exception via `NSAllowsLocalNetworking` (scoped to local networking, not `NSAllowsArbitraryLoads`), and the iOS 14+ local network privacy prompt via `NSLocalNetworkUsageDescription`; the pairing onboarding explains the prompt before it fires, because a declined prompt is indistinguishable from an unreachable daemon at the socket level (Apple TN3179).
- Android: cleartext traffic is blocked by default since API 28; the app ships a `network_security_config.xml` permitting cleartext only for the paired daemon host, never the global `usesCleartextTraffic` flag.
- None of this applies once the connection is `wss://`; the exceptions exist exactly as long as the LAN-plaintext tier does.

### 3.2 Off-LAN is an enabling dependency, not part of M7

True remote access is the TLS listener plus device keys per `security.md`, and it ships as its own work item (M6+), not inside M7.
M7's only obligation to that future: transport endpoints are data, not code.
A pairing stores a list of endpoints (LAN `ws://` now; relay or direct `wss://` later), and the client tries them in order; when off-LAN lands, existing pairings gain an endpoint without an app rebuild.
Everything in this spec that depends on off-LAN (true push, section 4.3) is explicitly marked as gated on it.

### 3.3 Reconnection, offline, and staleness

- Reconnection is the protocol's own story: `event.subscribe { cursor }` replays from the persisted cursor, so a dropped connection never loses timeline entries; terminal peeks get a fresh styled snapshot on catch-up per the protocol's backpressure design.
- `client-core` persists per-pairing: the event cursor, entity caches (cards, sessions, fleet snapshot), the Needs You queue, and the last styled terminal snapshot per recently-viewed session.
- Disconnected behavior: every surface renders the cached state **stale-marked** with a last-synced timestamp; nothing pretends to be live.
- All mutating actions (approve, merge, steer, resume, feedback submit) are disabled while disconnected; there is no offline queue by design (section 1).
- Artifact documents are not cached beyond the active review session; their signed URLs are short-lived by design and the cache would outlive its authorization.
- Backgrounding: the OS will suspend the app and kill the socket; foregrounding reconnects and replays from the cursor.
  This is the designed path, not an error path, and it must be visually seamless (no flash of empty state; cached content renders instantly, then live-refreshes).

## 4. Notifications

This section is deliberately honest, because the notification story is where mobile specs usually lie to themselves.

### 4.1 The problem statement

True push (delivery with the app killed and the screen off) requires three things at once:

1. **Sender credentials**: an APNs auth key and an FCM project, held by whoever calls Apple's and Google's push services.
   These cannot ship inside an open local-first app (a bundled APNs key is public, spoofable, and revocable), so real push implies a push relay operated by the project or self-hosted by the user.
2. **A sender with internet egress**: the daemon normally has outbound HTTPS ("LAN-only" constrains who can reach it, not what it can reach), so the daemon-to-relay leg is not the hard part.
3. **App-side receiver support**: and here Tauri has a real gap as of July 2026: **no official push notification support**.
   The tracking issue (tauri#11651, opened 2024-11) is still open; the official implementation PR (#11652) has sat in draft since early 2025.
   The most mature community plugin (Choochmeque/tauri-plugin-notifications: FCM, APNs, UnifiedPush) is v0.4.x with roughly sixty stars; usable for experiments, not something this spec builds a core promise on.

Meanwhile the OS baseline: iOS suspends a backgrounded app and its sockets within seconds to a minute, `BGAppRefreshTask` is opportunistic (minutes to hours, no guarantees), and Tauri's `disableBackgroundThrottling` option (iOS 17+) mitigates webview timer throttling, not process suspension.
Android, uniquely, allows a long-lived foreground service that can genuinely hold a connection with the screen off.

So the honest dependency chain is: **off-LAN infrastructure (TLS listener, device keys, relay) -> push relay -> platform push**, and none of that is M7.
M7 ships the tiers below that need no cloud at all.

### 4.2 What M7 ships

- **Tier 0 - foreground, both platforms.**
  While the app is open, live WS events drive in-app banners and the Needs You badge; the official notification plugin covers any OS-level surfacing needed (for example, alerts while the user is in a different tab of our own app).
  Works today, LAN-only, no cloud.
- **Tier 1 - Android connected mode.**
  A small in-repo foreground service (Kotlin, plus the shell process) holds a narrow `event.subscribe` stream using the device token from the Keystore and posts real OS notifications with the screen off or the app backgrounded.
  Android requires the service to show a persistent notification, and Android 14+ requires a declared foreground service type; battery cost is real but small for an idle LAN WebSocket.
  The mode is a user-visible toggle, and Tauri has no official background-service story, so this is custom shell code - the single largest native-code item in M7, sized accordingly.
- **Tier 1 - iOS: does not exist, and the UI says so.**
  There is no supported way to hold a socket in the background; the app surfaces exactly this: "iOS shows alerts only while DapperFlow is open; background alerts arrive with remote access" - honesty in the product, not just the spec.
- **Tier 2 - true push: explicitly gated, not M7** (section 4.5).

### 4.3 Ranking and fatigue

Notification fatigue is a bug (`product.md`), and the phone is the easiest place to create it.

- The phone notifies only on high-priority Needs You arrivals, using the same ranking the desktop notification policy uses, evaluated daemon-side so every client agrees on what is notification-worthy.
- Per-category toggles live on the phone (approvals, blocked agents, plan rounds, green PRs).
- Rounds already dedupe to a single digest item upstream (`product.md`), so the phone inherits digest behavior for free.

### 4.4 Notification -> deep link -> resolving surface

- Notification payloads carry the Needs You item id, card id, and a surface hint; the notification text carries the card title and item kind, never message content, terminal content, or anything the scrubber guards (lock screens and paired watches display notification text).
- Tapping fires the app's deep link (`dapperflow://needs-you/<item>`); the app foregrounds, reconnects, replays from its cursor, then resolves:
  - item still open: land directly on its resolving surface (plan chrome, approval sheet, terminal peek) - one tap, per the invariant;
  - item already resolved elsewhere: land on the card's timeline entry marked resolved; a notification tap never dead-ends.

### 4.5 The push seam (Tier 2, post-M7)

When off-LAN remote access exists (TLS listener, device keys, relay per `security.md`), push becomes buildable:

- The relay (project-hosted or self-hosted) holds the APNs/FCM credentials; the daemon sends it events over its authenticated channel.
- Payload policy per the local-first promise: content-free wake pings ("N items need you") or end-to-end encrypted payloads the relay cannot read; the choice is an open question (section 7).
- Receiver side: adopt the community plugin if it has matured, otherwise a small in-repo native receiver per platform, same policy as keystore storage.
- `packages/platform` already carries the push-registration slot returning `unsupported`, so Tier 2 changes no client architecture, only a platform implementation.

Nothing in M7a-c may take a dependency that makes Tier 2 harder; that is the whole design obligation push places on M7.

## 5. Security

### 5.1 Token storage

- The per-device capability token is stored in the platform keystore: iOS Keychain, Android Keystore-backed encrypted storage, via the platform seam (section 2.2).
- Accessibility class: after-first-unlock, this-device-only (no cloud keychain sync, no backup export); Android needs after-first-unlock because the connected-notifications service (section 4.2) reconnects without the UI.
- The token never appears in webview `localStorage`, `IndexedDB`, URLs, or logs; the webview receives it in memory via one shell IPC call at startup, mirroring the desktop's handoff rule (`protocol.md`).
- This is a headline M7 motivation: the M6 PWA structurally cannot do better than browser storage for its token.

### 5.2 Capability profile

- The phone token carries exactly the profile in `security.md`: Needs You, approvals, steering; terminals read-only; no vault access, no recipe install; secret-bearing streams excluded.
- Enforcement is daemon-side scope checking; the app UI merely declines to render affordances the token cannot exercise, and a scope-denied error is treated as a bug in the app, not a dialog for the user.

### 5.3 Biometric gate

- Destructive-forward actions require a fresh biometric confirmation via the biometric plugin: **merge** and **plan approve** by default.
- Steering, feedback batches, and session resume do not gate by default; the policy is configurable per-action, and the policy is edited on the desktop (the trusted device), not on the phone.
- A failed or unavailable biometric falls back to device credential (PIN/pattern/passcode) where the platform offers it; there is no in-app fallback secret.
- Honesty about what the gate is: the official biometric plugin returns pass/fail and does not bind keystore entries, so the M7 gate is an app-enforced UX guard against pocket and shoulder access, not a cryptographic control (the daemon cannot verify a biometric happened).
  The upgrade path is real, though: when off-LAN device keys land, approve and merge can require a signature from a biometric-bound keystore key (which our in-repo keystore plugin can create), turning the gate into a cryptographic control the daemon verifies.
  M7 documents the gate as the former and leaves the latter to the off-LAN milestone.

### 5.4 Revocation and loss

- Per-device revocation lives in desktop Settings (`security.md`); revoking closes the device's connections with the distinct auth close code, and the app responds by shredding the keychain token and all local caches, then showing the pairing screen.
- Honest limitation: a phone that never reconnects keeps its cached metadata (card titles, states, status notes; never secrets, which are excluded upstream by scope and scrubbing).
  Revocation guarantees the daemon stops talking to the device; it cannot reach into an offline phone.
- Lost-phone guidance in Settings says exactly this.

### 5.5 Update and skew

- Webview assets ship inside the app bundle; the app never loads application code from the daemon or any remote origin (also an App Store requirement).
- Daemon/app version skew is handled by protocol version negotiation; an app older than the daemon's minimum gets the structured `upgrade_required` close and shows an update screen.

## 6. Milestone slicing

Each slice ends with acceptance criteria verified end-to-end on real devices, per the roadmap's standing rule; emulators are for development, not acceptance.

### M7a - Paired shell

The thinnest possible native app that is already better than the PWA.

- Prerequisite (shared with M6, done once): extract the pnpm workspace packages of section 2.2, with the desktop app consuming them.
- Tauri mobile shell wrapping the shared mobile web app, both platforms buildable (Android from the Windows dev machine; iOS via the macOS build path chosen in section 7).
- In-app QR pairing via barcode-scanner; pairing-token redemption for a per-device token (3.1); in-repo Keychain/Keystore plugin storing it (5.1); deep-link scheme registered.
- Reconnect with cursor replay; stale-marked cache (3.3); LAN plaintext platform declarations (3.1).
- **Gating spike, first thing in the slice**: artifact iframe + signed URLs + postMessage SDK proven on a real iPhone and Android device (2.4); a failure here reroutes rendering to the fallback packaging before anything is built on top.

Acceptance:

- On real devices on the same LAN: pair by scanning the desktop QR, approve a plan round, and merge a green PR from the native app (M6 acceptance parity, now native).
- Kill and relaunch the app: no re-pair, no missed timeline events (cursor replay observed).
- Background the app 30+ minutes, resume: no blank webview, full state restore (tauri#14371 soak).
- Inspect webview storage on both platforms: the token is absent; the platform keystore holds it.

### M7b - Attention surface

The reason the app exists.

- Tier 0 notifications on both platforms; Tier 1 Android connected-mode foreground service (4.2).
- Notification deep links resolving to surfaces per 4.4, including the already-resolved path.
- Biometric gate on merge and plan approve (5.3); revocation handling with token and cache shredding (5.4).

Acceptance:

- Android, screen off, app backgrounded: a Needs You arrival raises an OS notification within 10 seconds, and its tap lands on the resolving surface in one tap.
- iOS, app foregrounded: the same arrival banners in-app within 2 seconds; iOS background behavior matches the documented honesty text, not more, not less.
- A merge cannot complete without a fresh biometric pass; cancelling the prompt cancels the merge.
- Revoking the device from desktop Settings kicks the phone on next connect, shreds token and caches, and shows the pairing screen.

### M7c - Native polish

- Touch-tuned artifact chrome: pinch/pan explore, touch-selection annotate on quote anchors, phone-viewport layout warnings observed round-tripping to an agent.
- Terminal peek performance: a 200x50 styled snapshot renders in under one second on a mid-range Android device.
- Platform navigation (back gesture, safe areas), haptics on Needs You arrival, app identity (icon, splash, name).
- Onboarding UX for the iOS local-network prompt (explain before it fires; recover when denied) and Android battery-optimization guidance for connected mode.
- Distribution packaging executed per the section 7 decision (store listings or signed sideload artifacts).

Acceptance:

- A Deep-dial plan review completes two annotation rounds and an approval entirely on a phone.
- A deliberately overflowing artifact produces phone-viewport `layout_warnings` that the authoring agent fixes without human help.
- A first-run user reaches a paired, working app without touching OS Settings, on both platforms.

### M7d - Push (gated, explicitly not M7 core)

Scheduled only when the off-LAN relay exists; listed here so M7a-c leave the seam clean (4.5).

- APNs/FCM receiver per platform; relay subscription lifecycle (including dead-endpoint handling); content-free or E2E payloads per the section 7 decision.

Acceptance (when it runs): with the app killed and the phone on cellular, a high-priority Needs You arrival notifies within seconds and deep-links into the resolving surface after unlock and reconnect.

## 7. Open questions (for Matt)

1. **Distribution**: App Store + Play Store from the start, or personal-first (TestFlight / signed APK sideload) with store submission deferred until push (M7d) makes the app broadly useful?
   Store presence costs review friction and an Apple fee but is the only path to "install it like a real app" for other users.
2. **Platform order**: Android-first is the pragmatic default: the Windows dev machine builds and deploys Android locally, practitioner evidence is strongest there, and the store-proof gap is on iOS.
   But an attention surface lives in the pocket you actually carry: if your daily phone is an iPhone, we need the macOS build answer (a Mac mini, a cloud Mac CI runner, or a borrowed machine) from day one regardless.
   Which phone do you carry, and does a macOS build host exist today?
3. **Biometric policy**: is merge + plan-approve the right default gate set?
   Should steering gate too?
   And what re-prompt window: every gated action, or a short grace period after a pass?
4. **Push relay stance** (decides whether M7d ever ships for store builds): when off-LAN lands, is a project-hosted push relay acceptable for a local-first product, or must self-hosting be the primary story?
   And on payload policy: content-free pings or E2E-encrypted content?
5. **Concertmaster on the phone**: chat-based dispatch would exceed the phone capability profile (it can shape and dispatch work).
   Keep the phone strictly an attention surface, or plan a profile expansion once off-LAN device keys make stronger per-device auth possible?

## 8. References

Research performed 2026-07-04; sources fetched live on that date.

Tauri 2 mobile maturity:

- Tauri 2.0 stable announcement (2024-10-02), including the mobile DX and plugin-coverage caveats: https://v2.tauri.app/blog/tauri-20/
- Tauri 2.0 RC post (2024-08-01), "production ready mobile applications ... NOW" alongside the overpromising admission: https://v2.tauri.app/blog/tauri-2-0-0-release-candidate/
- Current crate version (2.11.5, 2026-07-01): https://crates.io/crates/tauri
- iOS prerequisites (macOS + Xcode): https://v2.tauri.app/start/prerequisites/
- iOS developer-experience feedback thread: https://github.com/tauri-apps/tauri/discussions/10197
- Android practitioner retrospective, four working apps (2025-10-05): https://blog.erikhorton.com/2025/10/05/4-mobile-apps-with-tauri-a-retrospective.html

Plugins:

- Notification plugin (local notifications, all platforms; no push): https://v2.tauri.app/plugin/notification/
- Push tracking issue, open since 2024-11: https://github.com/tauri-apps/tauri/issues/11651
- Official push PR, draft since early 2025: https://github.com/tauri-apps/tauri/pull/11652
- Push request closed as duplicate: https://github.com/tauri-apps/plugins-workspace/issues/1698
- Most mature community push plugin: https://github.com/Choochmeque/tauri-plugin-notifications
- Deep-link plugin (static declaration caveat): https://v2.tauri.app/plugin/deep-linking/
- Stronghold plugin (file vault, not Keychain/Keystore): https://v2.tauri.app/plugin/stronghold/
- Biometric plugin (pass/fail only): https://v2.tauri.app/plugin/biometric/
- Community keystore plugins: https://github.com/impierce/tauri-plugin-keystore and https://github.com/Choochmeque/tauri-plugin-biometry
- Keychain/Keystore + biometrics implementation guide (2026-01-12): https://decentpaste.com/blog/cross-platform-biometric-keyring-storage-tauri/

Backgrounding and WKWebView:

- iOS webview blank on resume (open): https://github.com/tauri-apps/tauri/issues/14371
- `disableBackgroundThrottling` (WebKit-only, iOS 17+): https://github.com/tauri-apps/tauri/pull/12181
- Background sockets die on iOS (platform, not Tauri): https://developer.apple.com/forums/thread/66157
- Service workers unavailable in WKWebView without app-bound domains: https://github.com/tauri-apps/wry/issues/1587 (plumbing landed in wry PR #1588, 2025-07-16)
- Custom-scheme origin vs `frame-ancestors`: https://github.com/tauri-apps/tauri/issues/14056
- Custom-protocol iframe "requested insecurely" history: https://github.com/tauri-apps/tauri/issues/12767 and https://github.com/tauri-apps/tauri/issues/3543

Existence proofs and distribution:

- Verified Play Store Tauri app (Sarman Signage): https://play.google.com/store/apps/details?id=com.clohive.sarman
- Official distribution guides: https://v2.tauri.app/distribute/app-store/ and https://v2.tauri.app/distribute/google-play/
- Negative finding: Flying Carpet's mobile versions are Kotlin/Swift, not Tauri (frequently miscited): https://github.com/spieglt/FlyingCarpet

PWA-versus-native on iOS (and the secure-context wall):

- Web Push for home-screen web apps, iOS 16.4 requirements: https://webkit.org/blog/13878/
- Declarative Web Push, iOS 18.4: https://webkit.org/blog/16535/
- iOS 26: added sites open as web apps by default: https://webkit.org/blog/16993/
- Secure contexts exclude plain-HTTP LAN origins: https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Secure_Contexts and https://www.w3.org/TR/secure-contexts/
- Service workers require a secure context: https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API
- Android installability requires HTTPS; non-HTTPS gets a capability-less shortcut: https://web.dev/articles/install-criteria and https://web.dev/learn/pwa/installation
- No background sync/fetch in WebKit: https://caniuse.com/background-sync
- WebAuthn RP ID cannot be an IP address: https://github.com/w3c/webauthn/issues/1358
- iOS push subscription reliability reports: https://developer.apple.com/forums/thread/728796
- Home-screen web apps exempt from the 7-day ITP cap: https://webkit.org/blog/10218/ ; storage quota policy: https://webkit.org/blog/14403/
- EU home-screen web app reversal (2024-03-01): https://techcrunch.com/2024/03/01/apple-reverses-decision-about-blocking-web-apps-on-iphones-in-the-eu/ ; Apple's standing DMA position: https://developer.apple.com/support/dma-and-apps-in-the-eu/

Platform declarations for LAN plaintext:

- ATS local-networking exception: https://developer.apple.com/documentation/bundleresources/information-property-list/nsapptransportsecurity/nsallowslocalnetworking
- iOS local network privacy (TN3179): https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy and https://developer.apple.com/documentation/bundleresources/information-property-list/nslocalnetworkusagedescription
- Android cleartext policy and network security config: https://developer.android.com/privacy-and-security/security-config
- Chrome Local Network Access permission (the trend is tightening): https://developer.chrome.com/blog/local-network-access
