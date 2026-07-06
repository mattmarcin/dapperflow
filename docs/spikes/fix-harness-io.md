# Spike: harness launch + composer I/O + session self-termination (findings #2, #3, #5)

Making DapperFlow agent-agnostic on Windows: the three defects from the live 4-harness audit that made it claude-first.
Branch `fix/fix-harness-io`.
Live matrix machine: claude `2.1.200`, codex `codex-cli 0.142.5`, opencode `1.17.13`, pi `0.80.3`.

Before this work: claude passed end to end; codex, opencode, and pi were all broken.
After this work: all four launch, receive their first prompt, receive mid-session steering, and self-terminate.

## Finding #2 - launch error 193

### Root cause (proven)

On Windows only `claude` ships a native `claude.exe`.
`codex`, `opencode`, and `pi` install as an extension-less `#!/bin/sh` shim (fnm/npm) next to a `.cmd`:

```
codex    -> C:\Users\m\AppData\Local\fnm_multishells\...\codex        (extension-less shim)
            C:\Users\m\AppData\Local\fnm_multishells\...\codex.cmd
opencode -> ...\opencode  ...\opencode.cmd  C:\Users\m\scoop\shims\opencode.exe
pi       -> ...\pi        ...\pi.cmd
claude   -> C:\Users\m\.local\bin\claude.exe
```

The session spawn path handed the bare manifest command (`codex`) to portable-pty, whose Windows `search_path` (`cmdbuilder.rs`) checks the **exact** name in each PATH dir *before* trying `PATHEXT`:

```rust
let candidate = path.join(&exe);
if candidate.exists() { return candidate.into_os_string(); } // the extension-less shim wins
```

So portable-pty resolved `codex` to the extension-less shim and passed it to `CreateProcessW` as the module name (no further search), which failed with **os error 193** ("%1 is not a valid Win32 application").
A `.cmd` would fail the same way: it is not a PE image.
The detection path (`agents::resolve_on_path`) already resolved correctly because it tries `PATHEXT` first and returns the bare form only as a last resort.

### Fix

`agents::launchable_command` (dflow-core), called by `Session::spawn_with` before `CommandBuilder`:

- resolves `command[0]` on PATH preferring a launchable extension (`.exe`/`.cmd`/`.bat`/... via `PATHEXT`), never the extension-less shim;
- runs a batch shim (`.cmd`/`.bat`) through `cmd.exe /c <script> <args...>` because `CreateProcessW` cannot execute a batch script directly;
- uses a native executable directly; passes an unresolvable command through unchanged so the spawner surfaces the real error.

Unit tests (`agents.rs`): a PATH dir holding both a shim and a `.cmd` resolves to `cmd.exe /c ...codex.cmd`; a native `.exe` is used directly; an unresolvable command passes through.

### Live proof

`live_harness_io::live_four_harness_launch_and_first_prompt` dispatched all four; result: `LAUNCHED WITHOUT os error 193: ["claude", "codex", "opencode", "pi"]`.
Every session drew its TUI (banners, busy footers, model names), which an os-error-193 launch cannot do.

## Finding #3 - composer input only landed on claude

### Root cause (proven live, not assumed)

`live_harness_io::probe_typed_injection_root_cause` launched each harness interactively and sent one `session.send_verified`.
Before the fix, all three non-claude harnesses returned:

```
[codex]    send_verified result: {"attempts":0,"submitted":false}
[opencode] send_verified result: {"attempts":0,"submitted":false}
[pi]       send_verified result: {"attempts":0,"submitted":false}
```

`attempts: 0` means the code returned **before ever typing** - at the `wait_for_composer_ready` gate (`run_send_verified` returns early on a readiness timeout).
The composers were clearly present and idle (codex `> Explain this codebase`, opencode `Ask anything...`, pi's prompt), but the gate required `ComposerState::Empty` from `classify_composer`, which is claude-tuned:

- opencode's composer uses the heavy box-drawing prompt marker `U+2503` which is not in `PROMPT_MARKERS`, and its placeholder ghost styling / cursor row differ, so it never classified as `Empty`;
- codex shows the busy signature `esc to interrupt` while it loads MCP servers on startup, so it read as busy for the whole short (8s) window.

Result: the gate timed out, typing never fired, `$0` spent - exactly the reported defect.

### Fix (generic, plus manifest-driven knobs)

Generic (`steer::wait_for_composer_ready`): readiness is now harness-agnostic - alive, drawn, not busy, not a trust/permission dialog, confirmed across two consecutive polls, plus an optional positive `ready_signature`.
The `ComposerState::Empty` requirement is dropped.
The steering readiness window was widened (8s -> 25s) so codex's MCP-server startup settles.

Manifest-driven (`[composer]`, `manifest.rs`), so a harness-specific need is data, not code:

- `submit_key` = `enter` (CR, default) | `lf` | `crlf`;
- `paste_mode` = `none` (default) | `bracketed` (wraps typed text in an ESC[200~/ESC[201~ envelope so a multi-line prompt lands as one paste);
- `ready_signature` = optional positive readiness marker.

All four launch-set harnesses were proven to work with the defaults (`enter` / `none`), so their manifests are unchanged and ship the defaults.

### Launch-argument first prompt: evaluated and rejected for shim harnesses

The audit suggested preferring a launch-argument first prompt (deterministic) over typed injection.
It is not viable for the shim harnesses on Windows: `cmd.exe /c` truncates **any** multi-line argument at the first newline.
Probe `probe_cmd_multiline_arg` passed `"FIRST LINE\nSECOND LINE\nTHIRD LINE"` to a stub `.cmd` via the real spawn path; the shim received nothing past the first line (`MULTILINE ARG SURVIVED cmd.exe /c: false`).
The first live dispatch showed the same: opencode and pi received only the brief's first line (the card title `"opencode launch proof"` / `"launch proof"`) and asked what it meant.
Because the standing dflow guidance prepended to a New-Session first prompt is multi-line, a launch-argument prompt would arrive truncated on codex/opencode/pi.
The readiness-gated typed path carries the full multi-line prompt intact, so it is the delivery mechanism for the first prompt as well as steering.

### Live proof (first prompt AND steering, all four)

`live_harness_io::live_new_session_first_prompt_and_steer`: `session.create` with a single-line first prompt (guidance prepended, so the typed content is multi-line for codex/opencode/pi), then a mid-session `session.send_verified` steer.
Every harness received both and the agent replied with the requested word:

```
[claude]   first_prompt_queued=true  steer={"attempts":1,"submitted":true}   pineapple / watermelon
[codex]    first_prompt_queued=true  steer={"attempts":1,"submitted":true}   pineapple / watermelon
[opencode] first_prompt_queued=true  steer={"attempts":1,"submitted":true}   pineapple / watermelon
[pi]       first_prompt_queued=true  steer={"attempts":1,"submitted":true}   pineapple / watermelon
```

Transcript excerpts:

```
[claude]   > Reply with exactly the word: pineapple ...   ● pineapple      > Now reply ... watermelon   ● watermelon
[codex]      Reply with exactly the word: pineapple ...   • pineapple      > Now reply ... watermelon   • watermelon
[opencode] ┃ Reply with exactly the word: pineapple ...     pineapple      ┃ Now reply ... watermelon     watermelon
[pi]         Reply with exactly the word: pineapple ...     pineapple        Now reply ... watermelon     watermelon
```

No harness in the launch set needed `no_auto_steer`; the readiness fix alone made steering land on all four.
(`cursor` remains `no_auto_steer = true` pending its own audit, unchanged.)

## Finding #5 - sessions never self-terminate

### Root cause

The pty reader loop (`pump_reader`) finalizes a session only on reader EOF (`read` returns 0).
When the direct child (agent CLI) exits but a ConPTY descendant lingers holding the pseudo-console, EOF never arrives, so the session stays "alive" until an external deadline force-kills it.
In the daemon there is exactly one such deadline: `gate::wait_for_session_exit` (60s default), which force-killed a gate fixer mid-work in the live audit.

### Fix

A per-session watcher (`session::watch_child`) polls the **direct** child.
On its exit: a short grace period to flush final output, then tear down the whole tree via the per-session Job Object (which reaps the lingering descendant and, once the last pty client is gone, lets the reader reach EOF), and finalize the row `done` with the child's exit code (`store::finalize_session_note`).
The daemon-wide reaping job (`job.rs`) stays the outer safety net.
With the session self-finalizing, `gate::wait_for_session_exit` sees the session end and returns "finished" instead of timing out - no change to gate.rs needed.

### Deterministic test (no live CLI)

`session::tests::session_self_terminates_when_child_exits_with_lingering_descendant` (Windows): the direct child (PowerShell) detaches a `ping -n 120` in the same console (so it holds the pty), records its pid, then exits.
The session finalizes within the grace window (~1.3s, not at any timeout), the row is terminal with `ended_at` and an `exited (code ...)` note, and the lingering `ping` is confirmed reaped (pid no longer alive).
Stable across repeated runs.

## Files

- `crates/dflow-core/src/agents.rs` - `launchable_command` + resolver + tests (finding #2).
- `crates/dflow-core/src/session.rs` - spawn resolution wiring (finding #2); child-exit watcher, `teardown_tree`, exit-code finalize, deterministic test (finding #5).
- `crates/dflow-core/src/store/sessions.rs` - `finalize_session_note` (finding #5).
- `crates/dflow-core/src/steer.rs` - generic readiness gate, manifest submit key, bracketed paste (finding #3).
- `crates/dflow-core/src/manifest.rs` - `[composer]` `submit_key` / `paste_mode` / `ready_signature` + tests (finding #3).
- `crates/dflowd/src/api.rs` - widened steering readiness window (finding #3).
- `crates/dflowd/tests/live_harness_io.rs` - the live 4-harness matrix and probes.

## Discovered follow-up (out of the three findings' scope)

Dispatch delivers the composed brief as the `{prompt}` launch argument; on shim harnesses this truncates to the card title (same `cmd.exe` newline limit as above).
Recommended fix: deliver the dispatch brief for shim harnesses via the same readiness-gated typed path as the New-Session first prompt.
Documented in `docs/spec/adapters.md` (Dispatch flow, Known constraint).
