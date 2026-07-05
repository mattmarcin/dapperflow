# Product Specification

## Vision

DapperFlow is a native app for a developer who runs many projects and many AI coding agents at once.
Today that developer has a terminal window per project, loses track of which agent is stuck, and pays a heavy human context-switching tax.
DapperFlow replaces the pile of terminals with a single cockpit: the user conducts, agents perform, and the app routes the user's attention to exactly the things that need a human.

The product is a "harness for harnesses".
It is deliberately agnostic across agent CLIs (Claude Code, Codex, OpenCode, Pi at launch) so the user can follow model and harness quality wherever it moves, per task, without changing tools.

## Persona

The primary persona is a solo or small-team senior developer who:

- maintains several active repos at once (products, libraries, experiments),
- already pays for one or more agent CLI subscriptions,
- wants agents doing feature work, bug fixes, testing, and maintenance in parallel,
- refuses to babysit sessions or copy-paste context between terminals.

## Core concepts

### Project

A registered local git repository plus settings: default branch, delivery mode, check commands, and a project memory file the orchestrator maintains.

### Card

The unit of work on the board: feature, bug, chore, test, or investigation.
Cards can exist for work no agent touches (notes, someday items), so the board is a real project-management surface, not just an agent queue.
A card that gets dispatched gains sessions, worktrees, artifacts, and an event timeline.

### Board and columns

`Inbox -> Shaping -> Ready -> Performing -> Verifying -> Needs You -> PR -> Done`

- Inbox: captured, not yet shaped.
- Shaping: being turned into a dispatchable brief (possibly with the Concertmaster, possibly in Plan Studio).
- Ready: dispatchable; has a brief, a process dial setting, and a target project.
- Performing: one or more agent sessions actively working.
- Verifying: the gate pipeline is running (checks, adversarial review).
- Needs You: blocked on the human (see Attention Router).
- PR: pushed, PR open, CI running or green.
- Done: merged or closed.

Views support cross-project (default) and per-project filtering, with swimlanes by project.

Lane movement is event-driven: dispatch moves a card to Performing, a gate run to Verifying, an escalation to Needs You, a PR to PR, a merge to Done, all from `card_events`.
Manual drags are instructions, not decoration: dragging to Ready means "dispatch when ready", dragging an active card backward means "cancel or park" and asks for confirmation; automation never silently snaps a card back against a human move.

Cards with live sessions wear a **session strip**: harness glyph, color-coded lifecycle state chip, current recipe stage, elapsed time in state, and the agent's last tier-1 status note (e.g. "working: wiring reducer tests"), so progress is glanceable from the board without opening anything.

**One-click-to-terminal is a product invariant**: every representation of an agent - board card, Projects-view session row, Needs You item, Mission Control chip - opens its live terminal (or its card workspace focused on the Terminal tab) in exactly one click.

### Attention Router ("Needs You")

The single most important surface in the product.
A ranked, cross-project queue of every human-blocking item:

- a plan round awaiting feedback in Plan Studio,
- a gate finding requiring judgment,
- an agent that reported `blocked` or has been stuck past a threshold,
- a trust or permission dialog an agent hit,
- a green PR waiting for a merge decision.

Ranking factors: explicit card priority, staleness, and cost-of-delay heuristics (a blocked agent burning a worktree ranks above an unreviewed idea).
Each item deep-links into the exact card workspace tab that resolves it.
Desktop notifications mirror high-priority arrivals; notification fatigue is treated as a bug.

### Process dial and flow recipes

Every card carries a dial that selects a **flow recipe**: a shareable markdown file that defines how orchestration works for that card (stages, planning mode, harness axes, MCP mounts, gate strictness, ship target).
The bundled recipes are the default dial positions; the machinery is identical, only the recipe differs.

- **Presto**: dispatch immediately from a short brief; verify; PR. For obvious fixes.
- **Standard**: agent produces a lightweight plan artifact (one screen); user skims and approves or annotates once; implement; verify; PR.
- **Deep**: full Plan Studio loop with as many annotation rounds as needed; explicit plan approval; staged implementation; adversarial review. For features where design feedback matters.

Recipes make DapperFlow's opinions optional: users can skip HTML planning, mount outside MCP servers, change gate behavior, and share their flows as single files.
Full design in `recipes.md`.

### Environments

Worktrees inherit tracked files but not the environment that makes a project run (.env files, secrets, local services, ports).
DapperFlow's per-project Env Vault materializes vars, secrets, and env files into every leased worktree and shreds them at return; a port broker lets parallel dev servers of the same project coexist.
Full design in `environments.md`.

### Card sources: GitHub issue import

Cards can originate outside the app; GitHub Issues is the first source (modeled generically so Linear/Jira can follow as data, not schema changes).

- Per-project import config: assignee, label, and milestone filters, or a curated picker; never an unfiltered firehose into Inbox.
- An imported card lands in Inbox typed by label heuristics (bug/feature), carries `origin: github_issue` with the repo and number, and deduplicates on re-import (one issue, one card; re-import refreshes fields but respects local lane moves).
- The card workspace gains an **Issue tab** for origin cards: rendered issue body, labels, comments; delegation is then normal dispatch - pick a recipe and harness, go.
- Close-the-loop is free: when an origin card ships through the gate, the generated PR body includes `Fixes #<n>`, so GitHub closes the issue on merge; two-way status sync is deliberately deferred.

### Card sources: onboarding audit

One agent run that turns a cold repo into a warm project: seeded backlog, seeded knowledge, and a validated project profile (design: the design notes).

- **Offer, never force**: after `project.add` the app offers "Audit this project?" (quick scan or deep); the same action lives in the Projects view for re-audits. Never auto-runs.
- **Output contract**: budgeted (quick scan caps at 10 cards / 6 notes, deep at 25 / 12; overflow goes into the audit's own report, never the board); every card lands in **Inbox only** and must carry file:line evidence or a repro; completion raises exactly one `audit_digest` Needs You item with deep links; Inbox gains bulk triage (dismiss / send-to-Shaping).
- **Origin and dedupe**: cards carry `origin: audit` with a content fingerprint; re-audits refresh rather than refile, and durable dismissals suppress refiling.
- **Non-goals**: no auto-fixing during audit (the recipe has no ship stage, structurally), no auto-run on registration, no external writes.

### Session-first workflow (decided 2026-07-04)

**New Session is the front door.** The primary flow is: pick a project and an agent launcher, land directly in a live terminal, start talking.
No card, form, or brief is required to begin work.

Cards materialize from conversation: every dispatch brief instructs the agent to maintain the board through `dflow card` verbs (create, update, note, move) as a side effect of the work.
As you discuss a feature, the agent files the card, moves it to Performing, and keeps the status note current; the kanban becomes a live projection of conversations across all projects rather than a data-entry surface.

Multiplicity rules: a session may create and update **many** cards (file a bug it tripped over, split follow-ups), and a card may accumulate sessions over its life.
Only the worktree lease stays exclusive: one card owns a given checkout at a time.
Sessions may exist with no card at all (`sessions.card_id` nullable); the board simply does not show them (the Projects tree does).

The card form survives as the secondary path: backlog capture, GitHub issue import, and planned-ahead work that has no conversation yet.

### Sessions and terminals

Every agent runs in a real PTY rendered as a real terminal in the app.
The user can click into any session at any time and type; steering a working agent uses verified submit (see `adapters.md`) so injected messages reliably land.
Sessions survive GUI restarts because the daemon owns them.

### The Concertmaster

A first-class chat panel backed by a harness session of the user's choice with DapperFlow's MCP server mounted.
It can shape cards, dispatch work, summarize fleet state, maintain per-project memory, and answer "what is going on across my projects" from live data.
It is optional: every Concertmaster capability is also available through direct UI.

Principles (from the 2026-07-04 adversarial PM-layer review, the design notes):

- **Chat is a command surface, not a mediator.** The user is never required to go through the Concertmaster; deterministic routing (Attention Router) does the watching, and proactive findings land in Needs You, never as unsolicited chat.
- **Rounds**: scheduled or user-triggered headless Concertmaster runs (the Hermes heartbeat pattern, generalized from the Knowledge Gardener). Off by default, event-count gated, per-project schedulable. Judgment scope is only what deterministic routing cannot compute: cross-card synthesis, silence/drift detection, brief-quality feedback. Output contract: at most one deduplicated Needs You digest item per round, with deep links; threshold checks stay in the Attention Router.
- **Guarded steering**: one-shot only (never dialogue with a worker), bounded to the stuck-recovery playbook in adapters.md, attributed via a `concertmaster_steered` event and a terminal divider, rate-limited per session, `no_auto_steer` absolute, and zero authority transfer: merge, push, and discard stay human.
- **Scoped sessions instead of tiers**: a "per-project PM" is a Concertmaster session with a project filter and that project's knowledge digest as standing context - a parameter, not an architecture.
- **The one-click invariant extends to the Concertmaster's mouth**: every card, session, or finding it mentions renders as a one-click deep link, and its rounds and steers are themselves visible sessions and events.
- **Explicit non-goal: no PM hierarchy.** Re-visit triggers (recorded in the research doc section 6.4) are about standing-context divergence, never fleet size.

## UX views

1. **Mission Control**: fleet overview (all sessions with live state chips), the Needs You queue, and recent activity.
2. **Board**: the kanban described above.
3. **Projects view**: a persistent, expandable sidebar tree alongside the board (pattern validated by Codex Desktop's Projects sidebar).
   Each registered project (folder) expands to show: live agent sessions in it (with lifecycle state chips and elapsed time), resumable past sessions (most recent first, with first-prompt preview and relative age), and cards in flight for that project.
   Collapsed projects show badge counts (live sessions, Needs You items).
   Clicking a live session jumps to its terminal; clicking a resumable session resumes it (see architecture.md, session resume); clicking a card opens its workspace.
   The tree and the board are linked projections of the same entities: selecting a project in the tree filters the board to it, and selecting a session in the tree highlights its card on the board.
4. **Card Workspace**: opened from any card; tabs for Terminal(s), Plan (artifact chrome), Diff, and Timeline (event log).
5. **Concertmaster panel**: dockable chat sidebar, available everywhere.
6. **Settings**: projects, agents, adapters, tokens, templates, appearance.

### Configured agents (Settings > Agents)

Users run agents through **launchers**: configured entries that pair an adapter's behavior knowledge with the user's own command, arguments, and environment.

- **Autodetection**: on first run and on demand, DapperFlow scans PATH for known CLIs (claude, codex, opencode, cursor, pi) and creates enabled launchers for the ones found, with detected versions shown.
- **Custom launchers**: users add their own - name, base command, extra args, extra env vars - referencing an adapter family for behavior.
  The canonical example: `cc-alt`, a second Claude subscription, is the claude adapter family with a different config-dir env var; DapperFlow treats it as a first-class agent in every picker.
- **Extra args**: per-launcher default arguments (e.g. `--dangerously-skip-permissions`) appended at every launch; shown with a caution styling when they weaken safety.
- Every "pick an agent" surface (new session, dispatch, recipe axes) offers the configured launchers, not hardcoded harness names.
- Launchers persist in SQLite (see data-model.md `agents`); adapter families remain code+manifest (adapters.md) - launchers are user data, adapters are behavior.

Keyboard-first navigation throughout: a global command palette, single-key column moves, and jump-to-Needs-You.

## Product principles

1. No forced dependencies beyond git and the user's agent CLIs for the core experience (board, terminals, worktrees, plans); individual features carry their own opt-in dependencies (PR mode needs GitHub auth, declared services may need docker or wrangler, recipe MCP mounts need their commands) and degrade cleanly without them.
2. Real terminals, never a scraped chat facsimile.
3. Plans are interactive HTML artifacts, never terminal markdown.
4. The user's attention is the scarce resource being optimized.
5. Local-first; the user's code and data never leave the machine except through actions they take (push, PR).
6. Every automated claim is verifiable: timelines show evidence (exit codes, gate outputs, PR links), not vibes.
