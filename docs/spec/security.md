# Security Specification

The single threat model for DapperFlow.
Every other spec doc defers to this one for trust boundaries, secret handling, and sandboxing.
Added after the 2026-07-04 dual review (docs/reviews/) identified security as the suite's weakest area.

## Attacker classes

1. **Malicious repository content**: a cloned project whose files, hooks, or build scripts try to exfiltrate secrets or escape the worktree.
2. **Malicious or careless recipe**: shared markdown that mounts hostile MCP servers, disables the gate, targets `in_place`, or injects adversarial guidance into briefs.
3. **Hostile artifact**: agent-generated HTML in Plan Studio attempting network exfiltration, parent-DOM access, or SDK abuse.
4. **Compromised or misbehaving agent CLI**: a harness process that echoes secrets, writes outside its worktree, or abuses its `dflow` token.
5. **Local low-privilege process**: other software on the machine probing the daemon socket, token files, or state directories.
6. **Remote paired device (M6+)**: a stolen or coerced phone/web client.

## Token architecture

- **Root token**: minted on first daemon run, stored under the app data dir with owner-only ACL; grants full protocol scope; held by the desktop app.
- **Webview handoff**: the Tauri shell obtains the root token via one Tauri IPC call at startup and hands it to the webview once; the token never appears in URLs, query strings, or persisted frontend storage.
- **Per-task tokens**: minted at dispatch for the `dflow` CLI, scoped to that card's endpoints only, injected as env vars, revoked at teardown; a leaked task token cannot touch other cards, projects, or the vault.
- **Artifact URLs**: short-lived signed URLs (see Artifact sandbox) so artifact iframes never hold a bearer token at all.
- Rotation: root token can be rotated from Settings; rotation invalidates all client connections, which re-handshake.

## Secret handling policy

Secrets follow one lifecycle: vault (encrypted at rest) -> materialization (worktree env/file) -> use -> shredding at worktree return.
The boundaries beyond the vault API, previously unspecified, are:

- **Scrollback and VT snapshots**: any session whose environment received vault secrets gets write-time scrubbing: known secret values are pattern-replaced with `[dflow:redacted]` before scrollback persists to disk. The live screen is not scrubbed (the user may legitimately need to see what happened), only the durable ring files and captures that leave the session (peeks, gate evidence, event payloads).
- **Gate evidence and check output**: piped through the same value-matching scrubber before storage under `<app-data>/gate/`.
- **Event payloads and timelines**: producers must never place raw env values in `card_events`; the scrubber runs as defense in depth on payload writes.
- **Artifacts**: the layout-audit pass also scans artifact HTML for known secret values before registration and blocks with a finding on a hit.
- **Drift guard captures** (environments.md) show diffs with values masked; absorbing a change writes to the vault without displaying the value.
- Known limitation, stated honestly: value-matching cannot catch transformed secrets (base64, split strings). The scrubber is mitigation, not proof; the primary control remains scoping which cards get which vault entries.

## Recipe trust tiers

Recipes are inert text, but what they declare has operational consequences, so recipes are classified at validation time:

- **Standard**: stage selection, plan modes, harness/model/effort axes, guidance text, pooled worktrees, full gate. Installable and runnable with no extra consent.
- **Privileged**: any of `mcp` mounts, `worktree: in_place`, `verify.gate: none` when a ship stage is present, or `ship.target: local_merge`. Requires an explicit per-project grant listing exactly what is elevated (the full MCP command lines, the in-place target, the disabled gate), re-confirmed when the recipe file's hash changes. Shipless recipes (audit) with `gate: none` stay standard.
- Audit sessions carry the standard per-task scoped token with no lane-move grant on the cards they create: an audit can file into Inbox but can never advance its own filings.
- The UI labels privileged recipes distinctly on cards and in the recipe list; a hash change on any installed recipe is surfaced.
- recipes.md's sharing claim is bounded accordingly: sharing a standard recipe carries prompt-injection risk only; sharing a privileged recipe carries execution and delivery risk and is gated as above.

## Artifact sandbox architecture

Decision (2026-07-04, after verifying Tauri multiwebview is an unstable feature flag with open positioning/rendering bugs): artifacts render in a **sandboxed `<iframe>` inside the main app webview**, not a separate Tauri webview.

- `sandbox="allow-scripts allow-forms"`: no same-origin, no top-navigation, no popups, no downloads.
- Assets and the artifact document are served by the daemon's loopback HTTP endpoint via **short-lived signed URLs** (capability in the URL, no bearer token in the iframe); the same mechanism serves a future remote client.
- CSP on artifact responses: `default-src 'none'; img-src <artifact-origin> data:; style-src <artifact-origin> 'unsafe-inline'; script-src <artifact-origin>; frame-ancestors <app-origin>; connect-src 'none'; base-uri 'none'; form-action 'self'` (last two per spike 5).
- The sandboxed iframe has an opaque origin (no `allow-same-origin`), so postMessage origin checks are impossible by construction; trust = signed-URL source identity + strict schema validation on every message (spike 5 verified: tampered signed URL 403s, external references blocked at runtime).
- The review SDK and a bundled Mermaid build are injected server-side into the served document (never fetched by the artifact), and the SDK talks to the app only through postMessage with an allowlisted, versioned message schema validated on the app side.
- Export strips the SDK and inlines assets, producing plain portable HTML.

## Concertmaster capability scope (M4, designed now)

The Concertmaster's MCP token scope excludes merge, push, discard, and permission changes; those remain human-only actions through the UI.
Steering authority is capability-scoped to the recovery-ladder playbook (adapters.md), so a compromised or confused Concertmaster session cannot exceed it.

## Remote access trust model (M6, designed now; LAN-first per user decision 2026-07-04)

"Same protocol" is not a trust story; remote adds:

- **v1 is a LAN web app (PWA), not a native app**: an explicit opt-in LAN listener (separate from loopback, off by default), serving a mobile-tuned web client of the same WS protocol.
- **QR pairing**: the desktop shows a QR encoding the LAN URL + a phone-scoped capability token; scanning is the whole pairing flow. Default phone capability profile: Needs You, approvals, steering; terminals read-only; no vault access, no recipe install.
- LAN v1 ships without TLS (self-signed certs on a LAN are a UX dead end), stated honestly: the capability token is the gate, the listener is opt-in, and the threat model assumes a trusted home/office network; enabling it shows exactly this caveat. True remote (off-LAN) access requires the TLS listener + device keys and stays M6+.
- Secret-bearing streams (env endpoints, raw scrollback) are excluded from remote scopes entirely in the first remote release.
- Per-device revocation from Settings.

## Non-goals

- Defending against an attacker with the user's own OS account privileges (they can read the app data dir regardless).
- Sandboxing the agent CLIs themselves; harness-level permission modes remain the user's choice and DapperFlow surfaces, but does not override, them.
