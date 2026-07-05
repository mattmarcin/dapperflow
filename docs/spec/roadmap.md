# Roadmap

Milestones are sequencing, not budgets; each ends with acceptance criteria verified end-to-end and `the project notes` updated.

## M0 - Spikes (de-risk before building)

Findings recorded in `docs/spikes/`, one file per spike.

1. **PTY + VT screen model (Windows first)**: spawn real agent CLIs under portable-pty; mirror output into the candidate crates (`alacritty_terminal` frontrunner, `vt100` and `avt` alternates; `wezterm-term` excluded as deliberately unpublished); verify resize races, alt-screen, bracketed paste, OSC sequences, truecolor, mouse mode, wide glyphs, reattach with scrollback, cursor and styled-cell queries; pick the crate on evidence behind the `ScreenModel` trait.
2. **Harness signal audit**: for claude, codex, opencode, pi (current versions): hooks/notify/events/server APIs, headless modes, model/effort/autonomy flags, trust dialogs, resume; fill the capability matrix in `spec/adapters.md`.
3. **Verified submit**: against real TUIs, prove the popup-swallow, placeholder-expansion, and ghost-text cases are detectable and recoverable via our screen model.
4. **xterm.js over WS in Tauri**: webview connects directly to a daemon WS; confirm smooth full-screen TUI rendering and input latency; establish the binary framing.
5. **Artifact chrome feasibility**: sandboxed iframe in the Tauri webview with injected SDK; prove text-range anchoring, control capture, and the layout audit work under the strict CSP.

Acceptance: each spike produces a hard go/no-go artifact in `docs/spikes/`: pass/fail matrices against named harness versions, recorded PTY transcripts and screen-model diffs for spike 1, the filled capability matrix for spike 2, the verified-submit case table for spike 3, latency numbers and screenshots for spike 4, and a working CSP proof for spike 5.
Any no-go triggers a spec revision before M1.

## M1 - "Podium" (V1: board + embedded terminals)

- `dflowd`: single instance, auto-start, loopback WS + token auth, session persistence across GUI restarts.
- Project registry; board CRUD with all columns; card workspace (Terminal + Timeline tabs).
- Per card: lease worktree (basic pool), launch chosen harness in an embedded terminal, attach/detach/reattach.
- Lifecycle v0 via tier-3 screen heuristics; needs-input raises Needs You v0; desktop notifications.

- Projects view v0 in the sidebar: expandable project tree with live sessions and their states.
- Daemon-restart reconciliation v0: on startup, sessions whose PTYs are gone are marked `interrupted` and shown as such (resume itself lands in M2 with the adapters).

Acceptance: run three cards across two projects concurrently, each in its own worktree and terminal; kill and reopen the GUI mid-run with zero agent disruption; kill the daemon mid-run and confirm reconciliation marks sessions interrupted with no orphan processes; a permission prompt in any session appears in Needs You within seconds.

## M2 - "Ensemble" (orchestration depth)

- Adapter manifests complete with tier-2 native signals; verified-submit steering from the UI.
- `dflow` CLI v1 (`card`, `status`); tier-1 signals wired into supervision.
- Flow recipes v1: parser, validation, scoping, bundled `presto` and `standard` recipes; dispatch fully recipe-driven; per-recipe MCP mounts.
- Env vault v1: vars/secrets/files, lease-time materialization, teardown shredding, import assist.
- Event timeline UI; worktree pool hardening (locks, Job Objects, long paths).

Interim ship behavior (the gate engine does not exist until M5): M2 recipes run `verify` as `checks_only` (the project's registered check commands in a gate-class worktree), and any push requires an explicit user approval click; the bundled recipes ship with these settings until M5 upgrades them.

- Session resume end to end: `resume_ref` capture per harness (adapters.md table), `session.resume`, scrollback continuity divider, lineage chain; the ten live-probe items from spike 2 close here.

Acceptance: a card dispatched on `presto` with a vaulted `.env.local` runs checks green and reaches a user-approved pushed branch with every lifecycle transition visible in its timeline; a recipe edit changes dispatch behavior without an app rebuild; kill the daemon mid-conversation on claude and codex sessions, restart, and resume each with full conversational context from the Projects view.

## M3 - "Plan Studio"

- Artifact service; `dflow plan open/poll`; review chrome (annotations, controls, question keys, Mermaid, batch feedback); layout audit gate.
- Bundled `deep` recipe; plan approval flow; artifact playbooks injected into planning briefs.
- Onboarding audit v1: `audit` + `audit-deep` bundled recipes with enforced budgets, offer affordance after project.add and in the Projects view, origin/fingerprint dedupe, `audit_digest` Needs You item, Inbox bulk triage, check_cmds prefill from runbook frontmatter (the design notes).
- Env: drift guard; per-worktree services + port broker.

Acceptance: a Deep-dial feature completes two annotation rounds and an approval entirely inside the app; a deliberately broken artifact is masked and self-corrected by the agent from structured warnings; two parallel dev servers of one project run on brokered ports.

## M4 - "Concertmaster"

- `dflow-mcp` server (board CRUD, dispatch, fleet status, project memory, gate results); chat panel bound to a user-chosen harness session.
- Cross-project memory maintained by the Concertmaster; card shaping and dispatch from chat; fleet summarization from live data.
- Rounds v1: user-triggered floor check plus per-project schedule; gardener runs become a round type (knowledge.md).
- Guarded steering v1: recovery-ladder playbook only, per product.md's Concertmaster principles.

Acceptance: "what needs me right now?" and "take the top bug on acme-web and run it standard" both work from the chat panel on at least two different harnesses; a scheduled round detects a synthesized cross-card pattern and files exactly one deduplicated Needs You digest item with working deep links; a Concertmaster steer of a stuck session lands attributed in the card timeline behind a terminal divider; a `no_auto_steer` session refuses the steer with a Needs You explanation.

## M5 - "No Wrong Notes" (gate + GitHub)

Ordered so the read-only GitHub features land first:

1. GitHub transport via the local `gh` CLI (detected with `gh auth status`; `github.auth.*` verbs report gh presence/auth rather than running an OAuth flow).
2. Issue import per product.md (filtered/curated, origin cards, Issue tab in the card workspace) - read-only, ships before the gate; reuses the origin-dedupe semantics the onboarding audit proved in M3.
3. Gate engine per `spec/gate.md`: checks, adversarial cross-model review, autofix, escalation as Plan Studio finding reviews.
4. Push (git credential helper), PR via gh (with `Fixes #<n>` for origin cards), CI watch via `gh pr checks`, merge from board; teardown landed-work proofs.

Acceptance: a card travels Ready -> Performing -> Verifying -> PR -> Done with a real finding escalated and resolved in chrome; a reviewer harness different from the author catches a seeded bug; a GitHub issue is imported, delegated to an agent from its card, and auto-closes on merge via the PR's Fixes line.

## M6 - "Encore"

- LAN web app (PWA) as the v1 phone client: opt-in LAN listener, QR pairing with a phone-scoped capability token (Needs You + steer + approve, read-only terminals); TLS + device keys arrive with true off-LAN remote later.
- Sync groundwork over the event log; E2E-encrypted vault sync design.

Acceptance: pair a phone by scanning a QR from the desktop, then approve a plan round and merge a green PR from that phone on the same network.

## M7 - Native mobile app

Decision recorded 2026-07-04; full spec in `spec/mobile.md`, which re-verifies the claims below against current platform reality and slices the milestone (M7a-M7d).

- Default candidate: **Tauri 2 mobile** (iOS + Android are first-class targets since 2.0 stable; WKWebView / Android System WebView over the same Rust plugin model), so the M6 web client's code carries over nearly whole and the stack stays single-framework.
- What the native wrapper buys over the PWA: real push notifications (iOS PWAs remain second-class), OS keychain storage for the device token, app-store or sideload distribution, and a proper app identity on the phone.
- Known caveats to re-verify at spec time: mobile plugin coverage still trails desktop, the mobile dev experience is younger, and iOS builds require macOS + Xcode + an Apple developer account.
- Fallback if Tauri mobile disappoints at spec time: keep shipping the PWA, or a thin native shell (Swift/Kotlin WebView) around the same web client; a full React Native/Flutter rewrite is the explicit last resort since it forks the client codebase.
- The phone never bundles the daemon: it stays a pure client of the same authenticated WS protocol at every tier (PWA, Tauri mobile, anything else).
