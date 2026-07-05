# DapperFlow Mobile (M6 LAN PWA)

The phone attention surface: a mobile-tuned Progressive Web App that speaks the same authenticated WebSocket protocol (`docs/spec/protocol.md`) to `dflowd` as every other client.
It answers one question - "what needs me right now?" - and lets you resolve those items from the couch.
The desktop stays the only place work is shaped, dispatched, configured, and watched in depth.

This is the M6 predecessor of the M7 native app (`docs/spec/mobile.md`).
It is built **standalone** with a thin, duplicated protocol client (marked as debt in every copied file); M7 extracts the shared `packages/client-core` that all three shells will consume.

## What it does

Four thumb-first surfaces, scoped to the phone capability profile (`docs/spec/security.md`):

- **Needs You** (home): the ranked, cross-project attention queue; tap an item to open its resolving surface; approve or dismiss inline where the kind allows.
- **Fleet**: every live session with its lifecycle state chip, elapsed time, and last status note; tap for a read-only terminal peek.
- **Terminal peek**: a scrubbed styled screen snapshot rendered as monospace, poll-refreshed. No xterm, no keyboard, no steering - look, do not type.
- **Plan review**: a plan artifact read-only, plus approve with one overall feedback note (v1).
- **Settings**: connection state, pairing info, the capability profile made explicit, the honest no-TLS-on-LAN posture, and disconnect.

Merge of a green PR is shown as a **disabled preview** until the M5 delivery pipeline exists.

## Run it in dev (against fixtures)

```
pnpm install
pnpm dev
```

`pnpm dev` binds **0.0.0.0** on port **5273** (see `vite.config.ts`), so a real phone on the same LAN can load it.
Vite prints the reachable URLs; open the `Network:` one on your phone, e.g. `http://192.168.1.20:5273/`.

With no pairing, the app runs in **demo mode**: every view is populated from fixtures, so it fully demos with no daemon.
Settings has a demo control to toggle the Needs You queue between populated and all-clear.

### Windows firewall note

The first time Node binds `0.0.0.0`, Windows Defender Firewall usually pops a prompt - allow Node.js on **Private** networks.
If you dismissed it or use a stricter setup, open the port explicitly (elevated PowerShell):

```
New-NetFirewallRule -DisplayName "DapperFlow PWA dev 5273" -Direction Inbound -Action Allow -Protocol TCP -LocalPort 5273 -Profile Private
```

Remove it when you are done:

```
Remove-NetFirewallRule -DisplayName "DapperFlow PWA dev 5273"
```

## Pairing (go live)

The desktop shows a QR encoding a LAN URL plus a phone-scoped capability token:

```
http://<lan-ip>:<port>/m#pair=<base64url({ "url": "ws://<lan-ip>:<port>/ws", "token": "<phone-token>", "name": "<optional label>" })>
```

The OS camera opens that URL in the browser (the page itself cannot use the camera on a plain-HTTP LAN origin - the secure-context wall in `docs/spec/mobile.md`).
On load the app parses the `#pair=` fragment, persists `{url, token}`, strips the fragment from the address bar, connects as `client: mobile`, and switches to live data.
Disconnect from Settings shreds the stored pairing and returns to demo mode.

Honest limitation (`security.md` 5.1): a browser PWA has nothing better than `localStorage` for the token; it cannot reach the OS keychain the way the M7 native app will.

## Build

```
pnpm build     # tsc (strict) && vite build -> dist/
pnpm preview   # serve the built dist/ on 0.0.0.0:5273
pnpm icons     # regenerate the PWA PNG icons from the brand mark (public/)
```

Service worker registration is guarded to **secure contexts only** (`src/sw-register.ts`).
On a plain-HTTP LAN origin there is no service worker and no install prompt - by design - and the app is fully functional without one.
It registers where the origin is secure (localhost in dev, or a future TLS listener) purely for a faster app-shell cold load.

## Production integration seam (merge-time)

`vite.config.ts` sets `base: "./"`, so every asset reference in `dist/` is relative.
The daemon's opt-in LAN listener serves the built `dist/` under a path (the QR flow uses `/m`); because assets are relative, the bundle drops under whatever path the daemon mounts without a rebuild.
That path mount, plus the daemon-side pairing-token endpoint, is the single integration point where this client meets the real listener.

## Layout of the thin client (M6 debt)

- `src/client/` - duplicated protocol mirror (`protocol.ts`, `model.ts`) and a read-only WS client (`client.ts`).
- `src/data/` - one `MobileDataSource` interface with a `fixtures.ts` (demo) and a `live.ts` (real protocol) implementation.
- `src/pairing.ts`, `src/capabilities.ts` - pairing bootstrap and the phone capability profile.
- `src/views/`, `src/components/`, `src/state/` - the React surfaces and the app store.

Every duplicated file names the debt at the top and points to the M7 `packages/client-*` extraction that removes it.
