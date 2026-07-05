# Harness Adapters Specification

The agent-agnostic layer: how DapperFlow launches, supervises, steers, and retires sessions on any supported agent CLI.
Launch set: Claude Code (`claude`), Codex (`codex`), OpenCode (`opencode`), Pi (`pi`); Cursor's agent CLI is a detection candidate pending its own capability audit (Phase 2 checklist).
Cursor correction (2026-07-04, user-reported, verified against cursor.com/docs/cli/overview): the agent CLI command is `agent` with a `cursor-agent` alias; the `cursor` binary is the desktop editor and must never be detected as a launcher. Detection probes `cursor-agent` (unambiguous; bare `agent` is too generic).
Adding a harness must be data plus a probe run, never an engine change.

Adapters are behavior families; users launch through **configured launchers** (product.md, Settings > Agents): SQLite rows pairing an adapter with a user command, extra args, and extra env.
A launcher like `cc-alt` (second Claude subscription via a different config-dir env) inherits every claude adapter behavior - signals, verified submit, resume - while launching with its own credentials.
Dispatch resolves launcher first, adapter behavior second; unknown-adapter custom launchers get tier-1 + tier-3 supervision only and `no_auto_steer` by default.

## Adapter manifests

One TOML file per harness in `adapters/`.
Illustrative example (every value re-verified by the M0 probe suite before trust; prior-art observations seed the drafts but are never shipped unverified):

```toml
[adapter]
name = "claude"
command = "claude"
launch = ["{command}", "{autonomy_flags}", "{prompt}"]
autonomy_flags = ["--permission-mode", "acceptEdits"]
model_flag = ["--model", "{model}"]
effort_flag = ["--effort", "{effort}"]        # accepted set probed, not assumed
env = { CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION = "false" }

[signals]
busy_signature = "esc to interrupt"           # screen heuristic (tier 3)
native = "hooks"                              # tier-2 mechanism, probed in M0

[controls]
interrupt = "escape"
exit_command = "/exit"
resume = ["{command}", "--resume", "{resume_ref}"]
skill_invocation = "/{skill}"

[dialogs]
# pattern -> response, for trust/permission prompts detected on the screen model
trust = { pattern = "Do you trust", response = "enter" }

[composer]
# hints for verified submit: popup settle for prefix characters, ghost-text style
popup_prefixes = ["/"]
popup_settle_ms = 1200
ghost_text_styles = ["dim"]
```

## Three-tier signal model

Lifecycle states: `working | idle | needs_input | awaiting_feedback | blocked | done | error`.
`awaiting_feedback` is set from artifact status while the agent is parked on `dflow plan poll`; it suspends stuck detection so a plan review lasting hours never false-escalates.
Signals resolve top tier first; lower tiers corroborate or fill gaps.

1. **Tier 1 - explicit self-report**: the dispatch brief instructs the agent to call `dflow status working|blocked|done [note]` at meaningful boundaries.
   Most reliable, works identically on every harness, and carries intent ("blocked: need schema decision") rather than inference.
2. **Tier 2 - native harness signals**: whatever each CLI offers (hook systems, notify commands, turn/stop events, server APIs).
   Exact capabilities per harness are established by the M0 signal audit and recorded in the capability matrix below.
3. **Tier 3 - screen heuristics**: busy-signature match and composer-state classification on the daemon's VT screen model.
   Universal fallback; also the cross-check that catches a crashed agent that will never self-report.

Disagreement policy: a tier-1 `working` older than the stuck threshold is outranked by tier-3 evidence of an idle pane; a tier-3 busy match outranks a missing tier-2 event.

## Verified submit

The primitive that makes unattended steering trustworthy.
Sending text into an agent TUI can silently fail: slash/`$` popups swallow the first Enter, argument-hint placeholders expand instead of submitting, ghost text masquerades as typed input.

Algorithm (all reads against the daemon's own screen model):

1. Classify composer state (empty | has-text | popup-open), stripping ghost-text styles per manifest.
2. Type the text; if it starts with a popup prefix for this harness, wait `popup_settle_ms`.
3. Send Enter; re-read composer + cursor row.
4. If text remains pending (including placeholder-expanded text), retry Enter with backoff, bounded attempts.
5. Return `{ submitted, attempts }`; a failed submit raises `needs_you` instead of silently dropping the message.

Every adapter must pass the stub-TUI verified-submit test matrix (popup swallow, placeholder expansion, ghost text, slow redraw) plus an opt-in live-CLI smoke run, and ship composer-state test fixtures (recorded screen captures of empty, typed, popup-open, and placeholder states) so classification regressions are caught offline.
A manifest may declare `no_auto_steer = true` when its composer cannot be classified reliably; such sessions accept human typing but automated steering is refused with a Needs You explanation rather than attempted blind.

## Capability matrix (filled by M0 spike 2, harness signal audit, 2026-07-04)

Verification tags: `[L]` VERIFIED-LOCAL (command run on this machine, quoted in the spike file), `[D]` VERIFIED-DOCS (official doc URL cited in the spike file), `[S]` SEED (prior-art value, still needs a live-session probe in Phase 2).
Full evidence (command outputs, doc URLs, per-harness recommendation) lives in `the design notes`.

| Capability | claude | codex | opencode | pi |
|---|---|---|---|---|
| Version on this machine | `2.1.200` `[L]` | `codex-cli 0.142.5` `[L]` | `1.17.13` `[L]` | not installed, facts from docs only |
| Busy signature | `esc to interrupt` `[S]` | interrupt hint in TUI footer `[S]` | interrupt hint, double-Esc `[S]` | busy footer text `[S]` |
| Native turn/stop signal (tier 2) | Stop / SubagentStop / Notification hooks in settings.json, command or HTTP transport `[D]`; `--output-format stream-json --include-hook-events` `[L]` | `notify` program fires on `agent-turn-complete` (user-level config) `[D]`; `exec --json` JSONL event stream `[L]` | server SSE `GET /event` emitting `session.idle` `[D]`; plugin `session.idle` hook `[D]` | none native (no hooks, no notify); `--mode json` / `--mode rpc` event stream `[D]` |
| Headless/structured mode | `-p/--print` with `--output-format json\|stream-json`, `--json-schema` `[L]` | `codex exec --json` (JSONL), `--output-schema`, `-o` last-message `[L]` | `opencode serve` (:4096 HTTP+SSE), `run --format json`, `acp` `[L]`/`[D]` | `-p/--print`, `--mode json`, `--mode rpc` `[D]` |
| Model flag | `--model <alias\|full>` `[L]` | `-m/--model` `[L]` | `-m/--model provider/model` `[L]` | `--model provider/id`, `--provider` `[D]` |
| Effort flag + accepted set | `--effort low\|medium\|high\|xhigh\|max` `[L]` | `model_reasoning_effort minimal\|low\|medium\|high\|xhigh` via `-c`/config (no dedicated flag) `[D]` | `--variant` provider-specific (e.g. `minimal\|high\|max`) `[L]` | `--thinking off\|minimal\|low\|medium\|high\|xhigh` `[D]` |
| Autonomy flag | `--permission-mode acceptEdits\|auto\|bypassPermissions\|manual\|dontAsk\|plan`, `--dangerously-skip-permissions` `[L]` | `-a/--ask-for-approval untrusted\|on-request\|never` (on-failure deprecated) + `-s/--sandbox read-only\|workspace-write\|danger-full-access` `[L]` | `--auto` (auto-approve non-denied) `[L]` | `-a/--approve`, `-na/--no-approve` (no per-tool popups by design) `[D]` |
| Interrupt / exit / resume | Esc `[S]` / `Ctrl+C`,`/exit` / `-r/--resume`,`-c/--continue`,`--fork-session` `[L]` | Esc `[S]` / `Ctrl+C` / `codex resume --last`,`codex fork` `[L]` | Esc (double-Esc flaky `[S]`) / `Ctrl+C`,`/exit` / `-c`,`-s`,`--fork` `[L]` | Esc restores queued msgs / `/quit` / `-c/--continue`,`-r/--resume` `[D]` |
| Trust dialogs | workspace-trust dialog on first interactive run, skipped under `-p`/non-TTY `[L]`; exact text `[S]` | first-run folder-trust + approval-mode prompt `[D]`/`[S]` | permission prompts, no strong trust gate; `--auto` bypass `[D]`/`[S]` | trust prompt per folder to `~/.pi/agent/trust.json`, `/trust`, skipped in `-p`/json/rpc `[D]` |
| Skill invocation form | `/skill-name` (slash) `[L]` | `$skill-name` (skills), `/` for built-ins `[D]` | `/command` (slash) `[S]` | `/command`, `/skill:name` `[D]` |
| MCP mount (for Concertmaster) | `--mcp-config <files\|json>`, `--strict-mcp-config`, `claude mcp add` `[L]` | `codex mcp add` / config `[mcp_servers]` / `-c`; `codex mcp-server` exposes Codex itself as MCP `[L]` | `opencode mcp add` / config `mcp` block `[L]` | none, MCP intentionally excluded by design `[D]` |

Seed corrections learned in this audit (prior seed values re-verified against current CLIs and docs, 2026-07-04):

- claude busy `esc to interrupt` remains an unverified tier-3 seed (TUI footer text was not probed live per safety rules); the tier-2 hook path is far stronger and is now the recommended channel.
- codex skill form `$skill` is CONFIRMED by docs: skills invoke with `$` (`$skill-name`), while `/` opens built-in slash commands (`/mcp`, `/review`, `/status`); the popup-swallow hazard on `$` still needs a live verified-submit probe.
- opencode double-escape interrupt and background self-upgrade (`opencode upgrade`) remain hazards to probe live; the cleanest supervision channel is now the server SSE `session.idle` event, not the TUI.
- pi trust prompt per path is CONFIRMED (saved to `~/.pi/agent/trust.json`, managed via `/trust`) and positional-arg brief is CONFIRMED (`pi "brief"`); new correction: pi intentionally ships no MCP, no sub-agents, and no permission popups, so an MCP-mounting recipe must fail validation on pi.

## Standing-guidance injection (per harness)

Every session must receive the dflow usage contract as ambient context (agent-cli.md), injected the least-intrusive way each harness allows, and **never** by writing into the user's project checkout (a New Session runs with `cwd` = the user's real repo root).

Each adapter declares a `context_injection` method, resolved and verified per harness:

- **System-prompt append** (preferred): a launch flag or session setting that appends standing text to the system prompt for that session only (e.g. Claude Code `--append-system-prompt`, verified in the adapter probe). No repo pollution, no first-prompt edit, works for New Session.
- **Session settings file** (non-repo): a session-scoped config/instructions file passed by flag (the same mechanism the tier-2 hook `--settings` uses), living outside the worktree.
- **Worktree file** (dispatch/round only): for sessions whose `cwd` is a disposable leased worktree, a guidance file (`AGENTS.md`-style) may be dropped in the worktree - never for New Session, whose cwd is the user's checkout.
- **First-prompt fallback** (last resort): only if a harness offers no system-context mechanism, prepend a compact guidance preamble to the session's first message; documented as degraded.

The exact mechanism per harness is verified in the adapter probe suite before trust; an adapter with no non-polluting mechanism is flagged so New Session on it launches without standing guidance rather than writing into the user's repo.

## Resume-ref capture (for daemon-restart session resume)

The daemon must learn each session's harness-native id while the session runs, not at exit (a crash never reaches exit).
Capture mechanism per harness, researched 2026-07-04:

| Harness | Capture mechanism | Resume command | Notes |
|---|---|---|---|
| claude | Every hook event delivers `session_id` (and `transcript_path`); the daemon's HTTP hook endpoint captures it on the first Stop/Notification event `[D]` | `claude --resume <session-id>` (works even for sessions the interactive picker does not list) `[D]` | Resume assigns a NEW session id (anthropics/claude-code#12235), so re-capture from the resumed session's first hook event; stale ids after `/exit` are a known hazard (#9188), latest-wins by timestamp `[D]` |
| codex | `notify` payload for `agent-turn-complete` includes `thread-id` `[D]`; `exec --json` events carry it in headless mode `[L]` | `codex resume <thread-id>` / `codex resume --last` `[L]` | Payload also carries `last-assistant-message`, useful for the Projects view session preview `[D]` |
| opencode | server API lists sessions with ids; SSE events carry the session `[D]` | `opencode -s <session>` (`-c` for most recent) `[L]` | Server mode makes this the cleanest of the four |
| pi | session files on disk; id from `--mode json` events in headless `[D]`; interactive capture needs a live probe (Phase 2 checklist) | `pi -r/--resume` (picker) / `-c` most recent `[D]` | Interactive resume-by-id needs live verification |

`resume_ref` persists to SQLite the moment it is captured; a session with no captured ref yet is resumable only via the harness's own most-recent mechanism, which the UI labels honestly.

## Dispatch flow

1. Resolve recipe -> stages, harness/model/effort axes (card override > recipe > project default); validate recipe x harness compatibility (an MCP-mounting recipe on a harness without verified MCP support fails validation at dispatch, not mid-run).
2. Lease worktree.
3. Materialize environment (see environments.md): env files written, vars staged for the spawn environment (env can only enter a process at spawn, never injected afterward).
4. Start declared per-worktree services; port broker allocates and stages port env vars; a failed required service parks the card in Needs You instead of launching the agent.
5. Stage `dflow` onto PATH with the per-task scoped token env.
6. Compose brief: card brief + acceptance criteria + project memory digest + recipe stage guidance + `dflow` usage contract.
7. Launch per manifest; watch for trust dialogs within the first N seconds and answer per manifest; confirm the brief started processing.
8. Supervision loop consumes signals; state transitions append `card_events` and update Needs You.

## Steering and recovery

- UI steering and Concertmaster steering both go through `session.send_verified`.
- Interrupt/exit/resume use manifest controls; `resume_ref` is captured from harness output when available.
- Stuck recovery ladder: nudge (verified-submit a continuation prompt) -> interrupt + re-brief -> kill session, keep worktree, escalate to Needs You with full context.
- Concertmaster steers (M4) use this same `session.send_verified` path and this same ladder, adding only the `concertmaster_steered` attribution event and a per-session rate limit; `no_auto_steer` semantics are unchanged and absolute.
