# Spike: dflow in every session, with automatic standing guidance

Evidence file for the "dflow-everywhere" phase (branch `feat/dflow-all-sessions`).

Goal, verbatim intent from the user:
"clis need to be able to talk to dflow and understand how to use it.
I don't want to have to tell agents to use it.
It should have guidance and when and how to use it to keep the board up to date."

Contract: `docs/spec/agent-cli.md` ("Availability and standing guidance (all sessions)" and "The standing guidance content"),
`docs/spec/adapters.md` ("Standing-guidance injection (per harness)"),
`docs/spec/security.md` ("Token architecture" / Per-task tokens).

## Problem, as found

Before this phase, `dflow` env was injected at dispatch and at Concertmaster rounds, and the `dflow` usage contract was composed into the dispatch brief.
A plain New Session (Ctrl+N, `session.create` with no card) already minted a project-scoped token and injected `DFLOW_TOKEN` / `DFLOW_ENDPOINT` / PATH in the baseline (verified by reading `crates/dflowd/src/conn.rs` `session.create`), but it received NO standing guidance: nothing told the agent that `dflow` exists or when and how to use it.
So a New Session had the tool on PATH but was not self-explaining, and the user still had to tell the agent to use it.

The missing piece was the automatic "how and when": a way to inject the standing guidance as ambient context into every session through the harness's own system-prompt mechanism, never by writing a file into the user's project checkout (a New Session's cwd is the user's real repo root).

## Per-harness context-injection mechanism (verified)

Each adapter manifest now declares a `[context_injection]` method (`crates/dflow-core/src/manifest.rs`, `adapters/*.toml`).
Verification tags: `[L]` verified-local via `--help` on this machine, `[D]` verified-docs, `[S]` seed / unaudited.

| Harness | Session-scoped, non-polluting mechanism? | `context_injection.method` | Evidence |
|---|---|---|---|
| claude | Yes: `--append-system-prompt "<text>"` appends to the system prompt for that session; also `--system-prompt`. Works for an interactive session, no repo file. | `append_system_prompt`, flag `["--append-system-prompt", "{guidance}"]` | `[L]` `claude --help` (claude 2.1.200): "--append-system-prompt <prompt>  Append a system prompt to the default" and "--system-prompt <prompt>  System prompt to use for the session". Live-proven below. |
| codex | No. Its only non-repo instructions channel is the GLOBAL per-user `~/.codex/AGENTS.md`, which applies to every codex session and would clobber the user's own global config. No session-scoped system-prompt flag. | `first_prompt` | `[D]` OpenAI Codex docs (custom instructions = AGENTS.md, global override = `~/.codex/AGENTS.md`); no per-session flag. |
| opencode | No. `opencode --help` and `opencode run --help` (1.17.13) expose only `--prompt`; no `--system` / `--instructions` flag. The `instructions` array is in the GLOBAL `~/.config/opencode/config.json`, applying to every session. | `first_prompt` | `[L]` `opencode --help`, `opencode run --help` on this machine show no system-prompt flag. |
| pi | No. Only the GLOBAL `~/.pi/agent/APPEND_SYSTEM.md` (highest precedence, every pi session). No session-scoped flag. Not installed on this machine. | `first_prompt` | `[D]` pi docs; not installed to probe. |
| cursor | Unaudited detection candidate, `no_auto_steer = true`; its composer and first-prompt submit are untrusted. | `none` (flagged guidance-unsupported until the cursor audit) | `[S]` conservative until the Phase 2 cursor capability audit. |

Menu mapping (`adapters.md` / Standing-guidance injection):

- System-prompt append (preferred): claude, via `append_system_prompt`.
- Session settings file (non-repo): not needed; no harness required it (claude's flag is cleaner, others have no session-scoped file).
- Worktree file (dispatch/round only): not used for New Session, whose cwd is the user's checkout.
- First-prompt fallback (last resort, degraded, non-polluting): codex, opencode, pi.
- Guidance-unsupported (flagged, launches without standing guidance rather than polluting): cursor.

Honest limitations:

- codex / opencode / pi get the guidance only when the New Session has an initial prompt (the fallback rides in the first submitted message).
  A bare New Session with no prompt on those harnesses launches without standing guidance rather than writing into the user's repo.
- The first-prompt fallback puts the guidance in the visible conversation transcript, not the repo; it never touches the user's checkout.

## Engine

- `crates/dflow-core/src/manifest.rs`: `ContextInjectionSection { method, flag }`, validated (method in the allowed set; `append_system_prompt` requires a `{guidance}` placeholder), plus `Manifest::context_injection_flag` / `context_injection_method`.
- `crates/dflowd/src/api.rs`: `DFLOW_STANDING_GUIDANCE` (the compact standing-guidance content from `agent-cli.md`) and `apply_standing_guidance(harness, &mut argv) -> GuidanceInjection`.
- `crates/dflowd/src/conn.rs` `session.create`: applies the guidance for a command built for a known harness (an explicit raw command is left untouched), splicing the system-prompt flag or prepending the first-prompt preamble; also answers a first-run trust dialog per the manifest (as dispatch does) and logs the guidance mechanism.
- Dispatch and rounds are unchanged: their briefs already carry the `dflow` usage contract (dispatch) or a purpose-built escalation-only brief (rounds), so they are not double-injected.

## Token scope proof (cardless project-scoped token)

A cardless New Session mints a per-task token with `card_id = None`, `project_id = <cwd match>` (`security.md` / Per-task tokens).
The agent surface (`crates/dflowd/src/conn.rs` `dispatch_agent`) plus card ownership (`api.rs` `ensure_card_owned`) bound it to: create cards, write knowledge, self-report, read its project; and NOT move a foreign card, reach the vault, or kill a session.

Proven by `crates/dflowd/tests/dflow_everywhere.rs::new_session_injects_project_scoped_dflow_env`:

- `agent.context` resolves the cwd's project ("repo") with no card.
- `card.create` succeeds and the card lands in the token's project.
- The created card is adopted as the session's card (the fleet row's `card_id` is now the created card).
- `card.move` on a card the token did not create -> `forbidden`.
- `env.list` (vault) -> `forbidden`.
- `session.kill` -> `forbidden`.

Command: `cargo test -p dflowd --test dflow_everywhere` -> 2 passed.

## Cardless dflow CLI behavior

`crates/dflowd/tests/dflow_everywhere.rs::cardless_dflow_binary_surface` runs the real `dflow` binary with an EMPTY `DFLOW_CARD`:

- bare `dflow` -> "no card assigned (cardless session)" + a next step, exit 0.
- `dflow status working "<note>"` -> self-reports on the session (no card needed); the session row goes `working` with the note.
- `dflow card create` -> files a card AND adopts it; the session row's `card_id` is then set; bare `dflow` afterwards shows the adopted card.
- `dflow know add` / `dflow know find` -> project-scoped, write and locate a note in the project knowledgebase.

Note (Windows): a cardless session's `DFLOW_CARD` is the empty string, and Windows drops an empty-valued env var, so it arrives unset.
The CLI treats unset and empty identically (`non_empty_var`), so cardless behavior is correct either way.

## Live proof (approved): real claude on haiku, no manual dflow instruction

`crates/dflowd/tests/live_dflow_everywhere.rs::live_new_session_claude_haiku_has_dflow_and_guidance` (ignored by default).
Run against this worktree's own daemon build in an isolated `DFLOW_DATA_DIR` (never the user's running daemon); the session is kept short and kill-verified.

Command:
`cargo test -p dflowd --test live_dflow_everywhere -- --ignored --nocapture`
Result: `test result: ok. 1 passed` in ~84s.

What it did: registered a scratch project, added a `claude` launcher pinned to `--model haiku`, and opened a cardless New Session in that project with a TRIVIAL first prompt that never mentions dflow ("In two short sentences, what can you do in this session?").

Proof 1 (guidance is in the agent's context, deterministic):
the daemon log shows `standing dflow guidance: append_system_prompt` for the session, i.e. the launch was composed with `--append-system-prompt <standing guidance>`.

Proof 1b (behavioral, observed): asked only "what can you do in this session?", the haiku agent answered (first run, quoted from scrollback):
"I can help you with software engineering tasks ... and manage tasks on your DapperFlow board."
It knew about the board with no dflow mention in the prompt, purely from the injected system prompt.
(The model is non-deterministic; a later run gave a more generic answer, which is why Proof 1 is the deterministic anchor.)

Proof 2 (availability, deterministic): steering the session to run `dflow` yielded, verbatim in the TUI:
```
● no card assigned (cardless session)
  next: `dflow card create --title "..." --type <bug|feature|chore>` to file work as you discover it
```
So `dflow` is on the New Session PATH, authenticated with the injected project-scoped token over the injected endpoint, and reported the correct cardless surface.

Observation worth keeping: with the default permission mode, claude parks arbitrary bash (`dflow`) on an approval menu ("This command requires approval / Do you want to proceed?").
The agent recognized the command ("Run dflow to show current board status") before executing; the test answers the menu with Enter.

## What changed

- `crates/dflow-core/src/manifest.rs`: `[context_injection]` manifest section + validation + helpers + unit tests.
- `adapters/{claude,codex,opencode,pi,cursor}.toml`: each declares its `context_injection` method with the verification tag.
- `crates/dflowd/src/api.rs`: `DFLOW_STANDING_GUIDANCE`, `GuidanceInjection`, `apply_standing_guidance`; cardless `card.create` adopts the session's card; `agent.context` resolves the adopted card; unit tests.
- `crates/dflowd/src/conn.rs` `session.create`: apply standing guidance, answer the trust dialog, log the guidance mechanism.
- `crates/dflow-core/src/store/sessions.rs`: `set_session_card` (first card wins).
- Tests: `crates/dflowd/tests/dflow_everywhere.rs` (e2e), `crates/dflowd/tests/live_dflow_everywhere.rs` (live).

## Divergences and decisions

- Availability for New Session was already present in the baseline; this phase verified it with an e2e test rather than re-implementing it.
- The cardless token's scope was already correct (agent surface minus card binding, plus card-create); this phase added the adoption of the first created card so `dflow card create` "sets the session's card" per `agent-cli.md`, and covered the scope with an e2e test.
- Rounds keep their purpose-built escalation-only brief and do NOT receive the general standing guidance, deliberately: telling an escalation-only round to "create a card and start working" would contradict its remit.
  Dispatch keeps the contract in its composed brief (no double injection).
- OpenCode's `--system` flag (suggested by a community source) does NOT exist in 1.17.13 (`--help` verified), so opencode uses the first-prompt fallback, not a flag.
- cursor is flagged guidance-unsupported for New Session until its capability audit, rather than relying on an unprobed first-prompt path on a `no_auto_steer` harness.
- New Session now answers the first-run trust dialog (previously only dispatch did); this is the "make New Session consistent" part of the goal and is required for a New Session agent to get far enough to maintain the board.
