# Architecture Specification

## Two-process model

```
 phone/web client (future)          Tauri 2 desktop app (apps/desktop)
        \                               |  webview connects directly to the
         \                              |  daemon WS for PTY frames and artifacts;
          +---- authenticated WS -------+  Tauri IPC only for OS niceties
                        |
                 +-------------+
                 |   dflowd    |  Rust daemon, single instance, auto-started by the app
                 +-------------+
                   sessions | adapters | worktrees | env vault | recipes | artifacts | gate | store | MCP | GitHub
```

- `dflowd` owns all long-lived state: PTYs, VT screen models, worktree leases, the SQLite store, artifact sessions, gate runs.
- The GUI is a reconnectable client; closing or crashing it never kills agents.
- A future mobile/web client speaks the identical protocol (see `protocol.md`).

## Crate layout (cargo workspace)

| Crate | Kind | Responsibility |
|---|---|---|
| `dflow-core` | lib | Engine: session manager, VT screen model, adapter runtime, worktree pool, artifact service, store, event log |
| `dflow-proto` | lib | Versioned protocol types (serde), shared by daemon, desktop app, `dflow` CLI, and future clients |
| `dflowd` | bin | Daemon: axum/tokio WS server, auth, single-instance lock, lifecycle supervision |
| `dflow-cli` | bin | The agent-side `dflow` binary (see `agent-cli.md`) |
| `dflow-mcp` | bin/lib | MCP server exposing orchestration tools for the Concertmaster (rmcp) |

Frontend: `apps/desktop` is a Tauri 2 app, React + TypeScript, xterm.js for terminals, plus the Plan Studio artifact chrome.

## The internalized multiplexer

The single deepest design decision: everything tmux provided prior-art stacks reduces to (a) detached session persistence and (b) a queryable screen model.
Both live inside `dflow-core`:

- Each session = PTY handle (portable-pty) + a VT state machine fed every output byte + a scrollback ring persisted to disk (scrubbed per `security.md` when the session carries vault secrets).
- The VT crate lives behind a DapperFlow-owned `ScreenModel` trait; nothing outside that module touches the crate, so it stays swappable after M0.
- M0 candidates (verified 2026-07-04): `alacritty_terminal` is the frontrunner (published, maintained, used headless by Zed in exactly this pattern); `vt100` and `avt` are purpose-built screen-model alternates.
  `wezterm-term` is excluded: wezterm deliberately does not publish it and declares its API unstable.
- M0 acceptance for this layer must cover alt-screen transitions, resize races, bracketed paste, OSC sequences (titles, hyperlinks), truecolor, mouse mode, and wide glyphs against real agent TUIs.
- Native APIs replace tmux verbs: `capture(plain|styled)`, `cursor_pos()`, `screen_changed` subscription, `resize`, `send_bytes`, attach/detach with full state replay.
- Reattach after GUI restart replays screen state + recent scrollback instantly; no agent ever notices.

## Session lifecycle and supervision

- States: `starting -> working | idle | needs_input | awaiting_feedback | blocked | done | error` (see `adapters.md` for the three-tier signal model).
- `awaiting_feedback` is entered when the session's card has an artifact in `awaiting_feedback` status (the agent is legitimately parked on `dflow plan poll`); stuck detection is suspended in this state.
- A supervision loop in the daemon consumes signal events, debounces them, appends `card_events`, and updates the Needs You queue.
- Stuck detection: `working` with no screen change and no signal past an adapter-specific threshold escalates to Needs You.
- All child processes are tracked; Windows uses Job Objects so killing a session kills the whole tree; Unix uses process groups.

## Daemon restarts and session resume

The persistence story has three layers; the design goal is Codex-Desktop-grade continuity: restart anything, click a session, keep talking.

1. **GUI restart**: sessions are untouched; PTYs live in the daemon; reattach replays the screen (already covered above).
2. **Daemon restart (graceful or crash)**: ConPTY handles cannot outlive the daemon process, and children are deliberately tied to the daemon's Job Object on Windows so a dead daemon never leaves orphaned agents burning tokens invisibly.
   Recovery is therefore **harness-native session resume**: every supported CLI persists its own transcript on disk and accepts a resume flag (verified in spike 2: claude `--resume <session-id>`, codex `codex resume <thread-id>`, opencode `-s <session>`, pi `-r/--resume`).
   The daemon captures each session's harness-native id as `resume_ref` the moment it becomes known (capture mechanics per harness in adapters.md) and persists it in SQLite immediately, so even a crash loses nothing.
3. **On daemon startup**: reconciliation scans `sessions`; rows whose PTY process is gone are marked `interrupted` (never silently deleted); the Projects view and card workspaces show them as one-click resumable.

Resume mechanics:

- Resuming launches a fresh PTY in the same worktree with the harness resume flag; the harness reloads its own transcript, so the conversation continues with full context.
- A resumed session is a **new** `sessions` row linked via `resumed_from`; harnesses reassign ids on resume (verified for claude), so lineage is a chain, and `resume_ref` is re-captured from the new session's first signal (latest-wins by timestamp, guarding against stale hook events).
- The persisted scrollback ring from the predecessor renders above a "session resumed" divider in the terminal, so visual history is continuous even though the PTY is new.
- Graceful daemon upgrades use a drain mode: mark sessions, restart, then offer (or per-setting auto-run) resume for everything that was live.
- Worktree leases survive daemon restarts by construction (they are rows + directories, not process state); an `interrupted` session holds its lease until resumed or explicitly discarded.

## Worktree pool

- Per project, the daemon maintains a pool directory (default `%LOCALAPPDATA%/DapperFlow/worktrees/<project>/<slot>` on Windows, `~/.local/share/dapperflow/...` elsewhere).
- Lease on dispatch: pick a clean pooled worktree (caches intact: node_modules, target/, .next) or create one with `git worktree add --detach`.
- Return on teardown: refuse if there is unlanded work (committed but unpushed/unmerged); reset clean and mark available otherwise.
- One canonical state machine (`available | leased | dirty | retired`) governs every worktree, including gate worktrees; the landed-work proofs in `gate.md / Teardown safety` are the authoritative return conditions, and the dirty classification must handle staged files, untracked files, submodules, and LFS.
- Gate runs lease from the same pool as authoring (warm caches are exactly what checks need) but under a distinct `gate` lease class: never the authoring worktree, env materialized in checks-only mode, per-worktree services optional per project config.
- Windows hardening: long-path support, file-lock detection with actionable errors, no reliance on POSIX shell inside worktrees.
- Implementation shells out to system git (a hard dependency of the whole workflow anyway); no libgit2 to avoid worktree edge-case divergence.

## Security model

Owned entirely by `security.md` (threat model, token architecture, secret handling, recipe trust tiers, artifact sandbox, remote trust model).
Summary: loopback WS with a root bearer token (handed to the webview once via Tauri IPC); per-task scoped tokens for the `dflow` CLI; artifacts in a sandboxed iframe served by the daemon via short-lived signed URLs; write-time secret scrubbing on scrollback, gate evidence, and event payloads; remote access (M6) as a separate listener with device pairing and capability-scoped per-device tokens.

## Data and events

- SQLite via rusqlite (bundled), WAL mode; ULIDs for all ids; schema in `data-model.md`.
- All lifecycle mutations append to `card_events`; UI timelines, restart reconciliation, and the future sync layer all read the same log.

## Recipe engine and environments

- Dispatch is fully recipe-driven: the resolved flow recipe (see `recipes.md`) selects stages, planning mode, harness axes, MCP mounts, gate configuration, and ship target; the engine hardcodes no workflow policy.
- The env vault (see `environments.md`) materializes per-project vars, secrets, and env files into worktrees at lease time and shreds secrets at return; a port broker allocates real ports per service instance so parallel worktrees coexist.

## Platform notes

- Windows is the first validated platform (development machine), macOS and Linux enter CI at M1.
- ConPTY quirks (resize storms, alt-screen switching) get dedicated spike coverage before M1.
- The daemon is packaged with the app and auto-started/upgraded by it; it also runs standalone for headless/remote scenarios.
