# Spike: gate session briefs follow the adapter brief-delivery contract (not the argv path)

This lane closes the last flagged sibling of the brief-truncation bug: `gate::spawn_gate_session` still passed the reviewer/fixer brief as a launch argument.
Branch `fix/gate-brief-delivery`, based on `main` which already carries the dispatch brief-delivery fix (`docs/spikes/fix-brief-delivery.md`).
Live machine: claude `2.1.200` (native `claude.exe`), codex `codex-cli 0.142.5` (fnm `codex.cmd` shim).

## The exposure (proven)

The gate spawns two sessions on their target harness: an adversarial reviewer (`run_review`) and, for safe-mechanical findings, a fixer (`run_autofix`).
Both were launched by `spawn_gate_session`, which passed the composed brief as the `{prompt}` launch argument, exactly the shape the dispatch bug fixed.

On Windows only `claude` ships a native `claude.exe`; `codex`/`opencode` install as a `*.cmd` shim, so their session launch runs through `cmd.exe /c` (finding #2, `agents::launchable_command`), and `cmd.exe` truncates ANY multi-line argument at the first newline.
The reviewer brief (`compose_reviewer_brief`) is multi-line: line 1 is the reviewer preamble ("You are the adversarial reviewer ... on purpose."), and the diff, the acceptance/verify guidance, and the `dflow finding add` contract all sit BELOW that first newline.
So a reviewer on a shim harness received only its preamble - never the diff it was supposed to review, the acceptance criteria, or the contract for filing findings.

This is not an edge case: it is the DEFAULT reviewer path.
The gate REQUIRES the reviewer harness to differ from the author's (`reviewer_harness: different`, and the pipeline hard-fails an equal author/reviewer).
The default author is claude (native exe), so `resolve_reviewer_harness` picks the first available family that is not claude - codex or opencode, both shim harnesses.
The cross-model pairing the gate exists to create is exactly the pairing that put the reviewer on the truncating path.

## What the earlier live gate audit's codex reviewer actually received

The live gate audit (`docs/audits/gate-planstudio-live.md`, 2026-07-05) reported a real `codex` reviewer catching a real seeded off-by-one with a precise failure scenario ("`sumTo(5)` returns 10 ... omits the upper bound").
That reviewer received the FULL, untruncated brief - but NOT because the shim path worked.
The audit hit break W1 first: launching `codex` via its shim failed with `%1 is not a valid Win32 application` (os error 193, the pre-finding-#2 launch bug), so the audit **worked around it by repointing `DFLOW_LAUNCH_CODEX` at the native `codex.exe`** and running `codex exec ... {brief}`.
A native `.exe` is spawned by `CreateProcessW` directly, with no `cmd.exe /c` hop, so its multi-line argument is passed verbatim - the same reason claude is immune.
The reviewer caught the bug because it read the whole diff, and it read the whole diff because the audit had (unknowingly) taken it off the shim path entirely.

So the hypothesis "a truncated brief it compensated for via dflow context" is not what happened: the brief was not truncated, because the audit's workaround bypassed the truncating launch.
The audit's success is evidence the native-exe argv path works, not that the shim path is safe.
In default production - codex/opencode installed as `.cmd` shims, launched through the now-fixed finding-#2 `cmd.exe /c` path - the gate reviewer's brief WOULD have truncated to just the preamble line, and the reviewer would have reviewed blind.
That is the gap this lane closes.

## The fix

Route the gate brief through the SAME decision point dispatch uses, reusing the merged machinery rather than duplicating it.

`spawn_gate_session` now:

- resolves the launch, then calls `harness::brief_delivery(manifest, command)` (the shared decision - explicit manifest `brief_delivery`, else `auto` from the launchable form);
- **Argv** (native exe, claude - the default gate/fixer harness): the brief rides in as the launch argument, unchanged and proven;
- **Typed** (shim harness, codex/opencode - the default reviewer): re-resolves the launch with NO argv prompt (so nothing multi-line reaches `cmd.exe`), then, once the session is live, TYPES the full composed brief via the readiness-gated verified-submit path;
- on a failed typed submit, kills the briefless session and returns an error so the run fails/escalates honestly, never a silent empty-review pass.

### Shared helper (extracted from api.rs, not duplicated)

The typed-delivery core is extracted from `api::spawn_first_prompt_submit` into `api::deliver_typed_brief(session, prompt) -> bool`:
wait (readiness-gated) for the composer, then verified-submit, returning whether the brief landed.
Both callers now share one contract:

- `spawn_first_prompt_submit` (dispatch first prompt) wraps it on a background thread and raises Needs You on `false`;
- `spawn_gate_session` (gate reviewer/fixer) calls it synchronously and escalates the run on `false`.

The caller decides what a `false` return means, so a brief that never landed is never silently treated as delivered - the same principle the dispatch lane used.

### Honest failure wiring

A reviewer that never got its brief files no findings, and an empty review must never count as a pass (the trap the autofix audit already flagged for a different stage).
On a typed-delivery failure `spawn_gate_session` returns `Err("reviewer brief delivery failed")` (or `"fixer ..."`); `run_review` records a `gate_step` review `failed` with the reason and propagates, and `run_pipeline` finishes the run FAILED with that reason on the `gate_failed` event plus a `gate_finding` Needs You.
The run errors honestly rather than passing on a finding-less review.

### Adjacent fix required by the live path: trust dialog on gate sessions

A gate worktree is a fresh checkout, so a shim reviewer/fixer parks on a folder-trust prompt on first launch, and the readiness gate would time out on the dialog before it could type the brief.
Dispatch already answers this per the manifest (`spawn_trust_watcher`, `adapters.md` dispatch flow step 7); the gate did not.
`spawn_gate_session` now spawns the same trust watcher (a no-op for stub/no-manifest harnesses), so a real shim reviewer reaches its composer.
This is parity with dispatch, needed for the fix to actually work with a real shim harness.

## Deterministic tests (`crates/dflowd/tests/gate_brief_delivery.rs`, Windows-gated)

Both drive the real `gate.run` pipeline with a shim reviewer (following `crates/dflowd/tests/brief_delivery.rs`).

- `gate_reviewer_brief_reaches_a_shim_reviewer_below_the_fold`: the reviewer harness is `opencode` (declares typed) pointed at a `cmd.exe /c` shim launching the in-repo stub TUI, which mirrors its typed input to a capture file.
  A distinctive below-the-first-newline token from the reviewer brief's verify guidance reaches the reviewer's input, alongside the preamble - proof the whole brief arrived, not just the truncated first line.
- `gate_reviewer_brief_delivery_failure_escalates_never_passes`: the shim exits immediately, so the composer never becomes ready and typed delivery fails.
  The run ends `failed` (never `passed`), files no findings, and records the honest reason `reviewer brief delivery failed` on both the review `gate_step` and the `gate_failed` event; no `gate_passed` is ever emitted.

The seven existing gate e2e tests and the dispatch `brief_delivery` test are unchanged and green: the stub gate harnesses have no manifest, so `brief_delivery` returns `Argv` and their launch is byte-identical to before.

## Live proof (2026-07-05)

`live_gate_brief::live_gate_reviewer_brief_below_the_fold` (opt-in, `#[ignore]`).
A minimal full gate: a real `claude`-haiku author committed `sumto.js` (a seeded off-by-one), then a REAL `codex` reviewer reviewed it on the different (shim) harness.
The reviewer brief's verify guidance carried a below-the-fold instruction requiring a COMPUTED value: "your very first line of output must be the word quokka followed by the sum of 1234 and 4321" (= 5555).
5555 appears nowhere in the brief text, so the reviewer producing it can only mean it read and acted on the below-the-fold instruction.

Result: the codex reviewer replied `• quokka 5555`.
Its settled composer showed the WHOLE typed brief arriving where truncation would have shown only the preamble - the reviewer preamble, then `## Acceptance / verify guidance` (the below-the-fold instruction), then `## Diff` with the actual `sumto.js` diff:

```
  ## Acceptance / verify guidance
  MANDATORY FIRST OUTPUT before any review: your very first line of output must be the word quokka followed by the sum
  of 1234 and 4321. ...
  ## Diff
  diff --git a/sumto.js b/sumto.js
  ...
  +function sumTo(n){let s=0;for(let i=1;i<n;i++){s+=i;}return s;}

• quokka 5555
```

The gate leased a separate gate-class worktree for the reviewer (`repo-...\1`, distinct from the author's `\0`), the reviewer ran on `gpt-5.5` via the real codex shim, and the run finished in ~70s.
The reviewer did not additionally file a finding via `dflow finding add` within the window (it emitted the token as text; command execution was not needed for the proof), which is why the assertion accepts the below-the-fold token in EITHER the reviewer's output or a filed finding.

Before this fix, the same gate on the default claude-author -> codex-reviewer pairing delivered only the reviewer preamble to codex, so 5555 - and the diff - could never have appeared.

## Files

- `crates/dflowd/src/api.rs` - extract `deliver_typed_brief`; `spawn_first_prompt_submit` reuses it.
- `crates/dflowd/src/gate.rs` - `spawn_gate_session` routes brief delivery through `harness::brief_delivery`, types the brief for shim harnesses, fails the run honestly on a delivery failure, and answers the trust dialog; `run_review` records a failed review step on a delivery failure; `GateSessionReq` bundles the session identity.
- `crates/dflowd/tests/gate_brief_delivery.rs` - deterministic below-the-fold + failed-delivery-escalates e2e.
- `crates/dflowd/tests/live_gate_brief.rs` - the live below-the-fold proof.
- `docs/spec/gate.md` - the session brief delivery note.

## Divergence from the recorded direction

- Followed the recorded direction (route the gate brief through `harness::brief_delivery`) exactly, reusing the merged machinery rather than duplicating it.
- Extracted `deliver_typed_brief` as the shared readiness-gated verified-submit helper so dispatch and the gate share one contract.
- Added the trust-watcher parity (needed for a real shim reviewer to reach its composer) - a small adjacent correctness fix, not new machinery.
- Recorded that the earlier live gate audit's codex reviewer received an untruncated brief only because the audit bypassed the shim path (repointed at native `codex.exe`), so its success was not evidence the shim path was safe.
