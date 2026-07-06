# Spike: dispatch brief delivery - typed for shim harnesses, not truncated on cmd.exe

The dispatch brief was truncating to the card title on shim harnesses.
Branch `fix/brief-delivery`.
Live matrix machine: claude `2.1.200`, codex `codex-cli 0.142.5`, opencode `1.17.13`.

This lane fixes the follow-up recorded at the end of `docs/spikes/fix-harness-io.md`: dispatch delivered the composed brief as the `{prompt}` launch argument, and on a shim harness that argument is truncated.

## The bug (proven)

On Windows only `claude` ships a native `claude.exe`.
`codex`, `opencode`, and `pi` install as a `*.cmd` shim, so their session launch runs through `cmd.exe /c` (finding #2, `agents::launchable_command`).
`cmd.exe` truncates ANY multi-line argument at the first newline.

The composed dispatch brief is multi-line: card title, then card brief, acceptance criteria, project memory digest, recipe stage guidance, and the `dflow` usage contract (`api::compose_dispatch_brief`).
It was passed as the `{prompt}` launch argument, so a dispatched codex/opencode/pi agent received ONLY the first line - the card title.
It never saw its acceptance criteria, the recipe protocol, or the `dflow` contract.

Evidence recorded in the harness-io lane (`docs/spikes/fix-harness-io.md`, "Discovered follow-up" and the `probe_cmd_multiline_arg` probe): a multi-line argument reached a stub `.cmd` empty past the first line (`MULTILINE ARG SURVIVED cmd.exe /c: false`), and the first live dispatch showed opencode/pi receiving only the card title.
claude was unaffected: `claude.exe` is a native PE image, launched directly with no `cmd.exe` hop, so its multi-line brief arrives intact on the launch argument.

## The fix

A single delivery-mode decision, made once per dispatch: `harness::brief_delivery(manifest, command) -> BriefDelivery::{Argv, Typed}` (`crates/dflow-core/src/harness.rs`).

- **Argv**: the brief rides in as the `{prompt}` launch argument.
  Correct for a native exe (claude); proven and left unchanged.
- **Typed**: the brief is delivered by TYPED injection AFTER launch, via the same readiness-gated verified-submit path New-Session first prompts use (finding #3).
  For a typed harness, `dispatch_start` resolves the launch with NO argv prompt (so nothing multi-line is handed to `cmd.exe`), then, once the session is live, types the full composed brief through `spawn_first_prompt_submit`.
  A failed submit raises Needs You (`raise_first_prompt_failure`) rather than leaving a briefless agent running.

### The decision point

The mode is manifest data: `adapter.brief_delivery = "argv" | "typed" | "auto"` (`crates/dflow-core/src/manifest.rs`), default `auto`.

- `claude.toml` declares `argv` (native exe, proven).
- `codex.toml` / `opencode.toml` / `pi.toml` declare `typed` (shim launch truncates a launch arg).
- `auto` (an unspecified or custom manifest) decides from the resolved launchable form, reusing the finding #2 `agents::launchable_command` signal: a launch that resolves through `cmd.exe` is typed, a native executable is argv.
- An unmanifested custom/stub launcher has no verified-submit path, so it keeps argv.

Declaring the launch set explicitly (rather than relying only on `auto`) keeps the per-harness contract legible and stable regardless of PATH resolution order, while `auto` gives a sensible, self-correcting default for anything new.
The `{prompt}` slot is threaded as `Option<&str>` through the launch builders, so a typed harness resolves with `None`, which cleanly drops the slot and any dangling value flag (opencode's `--prompt`) instead of emitting an empty argument.

This matches the fix direction recorded in `docs/spikes/fix-harness-io.md` ("deliver the dispatch brief for shim harnesses via the same readiness-gated typed path used for the New-Session first prompt").

## Delivery-fidelity finding: ConPTY drops a lone LF on input

Probed directly against the stub TUI through a real PTY (both `paste_mode = none` and `paste_mode = bracketed`): a lone LF written to the ConPTY input channel does not reach the child.
So typed injection delivers all CONTENT but normalizes the inter-line newlines - adjacent lines are concatenated, the blank-line paragraph breaks collapse.

This is a property of keystroke injection through ConPTY, not of this fix, and it is shared with the already-proven New-Session first-prompt typed path (finding #3, where a multi-line standing-guidance preamble was delivered the same way and the agents replied correctly).
The bug this lane fixes is total truncation to the card title (content LOST past the first newline); typed delivery restores every line's content.
Newline fidelity (preserving the paragraph structure) would require a harness that honors bracketed paste with CR-as-soft-newline, a larger change that would also touch steering; it is out of scope here and noted as a possible future enhancement.

## Deterministic tests

- Delivery-mode decision (`crates/dflow-core/src/harness.rs` tests): an explicit `argv`/`typed` manifest field wins; `auto` reads the launchable form (a `.cmd` shim -> typed, a native `.exe` -> argv, Windows-gated); no manifest -> argv; the `Option<&str>` prompt threading drops the slot and the dangling `--prompt` when `None`.
- Manifest field (`crates/dflow-core/src/manifest.rs` tests): the launch set's declared modes, the `auto` default, and rejection of an invalid value.
- Stub-TUI shim launch (`crates/dflow-stubtui/tests/brief_delivery.rs`): a `.cmd` shim (rewritten to `cmd.exe /c` by `launchable_command`) launches the stub TUI; a multi-line brief typed through the readiness-gated verified-submit path lands in the stub's capture file with the card title AND every below-the-fold token present (no content lost past the first newline).
  The stub TUI (`crates/dflow-stubtui`) mirrors received input to `DFLOW_STUB_CAPTURE` for this assertion.
- E2E dispatch (`crates/dflowd/tests/brief_delivery.rs`): drives the real `dispatch.start` pipeline with the stub TUI behind a `cmd.exe /c` shim (opencode adapter, typed); a below-the-fold acceptance token from the composed brief reaches the agent's input.

## Live proof (2026-07-05)

`live_harness_io::live_dispatch_brief_below_the_fold` (opt-in).
A real card was dispatched on each harness; its acceptance criteria sat below the first newline and required COMPUTING a value - "reply with the word quokka followed by the sum of 1234 and 4321".
The answer, 5555, appears nowhere in the brief text, so an agent can produce it only by reading and acting on the below-the-fold instruction.

| Harness | Model | Delivery | Reply | Below-the-fold acted on |
|---|---|---|---|---|
| claude | haiku | argv (control) | `● quokka 5555` | yes |
| codex | (default) | typed | `• quokka 5555` | yes |
| opencode | opencode-go/glm-5.2 | typed | `quokka 5555` | yes |

All three replied `quokka 5555`.
codex and opencode are the fixed TYPED path; claude is the unchanged native-exe argv control.
The opencode session's composer additionally rendered the full multi-line brief (the acceptance line and the `dflow` contract visible in its input box), a direct view of the whole brief arriving where truncation would have shown only the title.

Before this fix, the same dispatch on codex/opencode/pi delivered only the card title (harness-io lane evidence), so the agent could not have produced 5555.

## Files

- `crates/dflow-core/src/manifest.rs` - `adapter.brief_delivery` field, constants, validation, tests.
- `crates/dflow-core/src/harness.rs` - `BriefDelivery`, `brief_delivery`, `launches_via_cmd_exe`; `Option<&str>` prompt threading; tests.
- `crates/dflowd/src/api.rs` - dispatch delivery decision and typed injection after launch; round path resolves with `None`.
- `adapters/{claude,codex,opencode,pi}.toml` - declared `brief_delivery`.
- `crates/dflow-stubtui/src/main.rs` - `DFLOW_STUB_CAPTURE` input mirror.
- `crates/dflow-stubtui/tests/brief_delivery.rs` - shim-launch typed multi-line delivery.
- `crates/dflowd/tests/brief_delivery.rs` - E2E dispatch delivery.
- `crates/dflowd/tests/live_harness_io.rs` - the live below-the-fold acceptance.
- `docs/spec/adapters.md` - the delivery contract (replaces the truncation caveat).

## Divergence from the recorded fix direction

- Followed the recorded direction (typed delivery for shim harnesses) exactly.
- Added a first-class manifest field `brief_delivery` (argv/typed/auto) so the per-harness contract is data and legible, with `auto` reusing `launchable_command` as the harness-io lane suggested.
- Discovered and documented the ConPTY lone-LF normalization: typed delivery is content-complete but collapses inter-line newlines, the same as the New-Session path. Not a regression; the truncation bug is fixed.

## Identified sibling (out of scope)

`gate::spawn_gate_session` still passes the gate reviewer/fixer brief as a launch argument, with the same shim truncation exposure.
The gate harness is claude (native exe) by default, so it is unaffected today; routing the gate brief through `harness::brief_delivery` is a tracked follow-up, deliberately not bundled into this dispatch-brief-delivery fix.
