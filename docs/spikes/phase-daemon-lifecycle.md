# Spike: daemon lifecycle overhaul

Evidence and design notes for the work specified in `docs/spec/daemon-lifecycle.md`.
Windows-first; macOS/Linux carry documented stubs until they enter CI at M1.

## Deliverable 1 - total reaping (the foundation)

### The bug this exists to kill

Force-killing `dflowd` repeatedly orphaned ConPTY console hosts (`OpenConsole.exe`) that
then busy-looped and burned CPU (100+ leaked processes, 1.4M CPU-seconds once).
The prior design assigned each session's *shell* to a per-session Windows Job Object, but
the ConPTY *console host* was never in any job, so an abrupt daemon death (crash or
`taskkill /F`) left it running with no owner.

### Why the host escaped the old job

portable-pty 0.9's ConPTY backend spawns two processes, neither of which the old
per-session job captured as the host:

- `crates/.../portable-pty-0.9.0/src/win/psuedocon.rs` `PsuedoCon::new` calls
  `CreatePseudoConsole(...)`. On modern Windows that launches the console host
  (`OpenConsole.exe`/`conhost.exe`) as a child of the **calling process** - the daemon,
  not the shell. portable-pty never exposes that host's pid.
- `PsuedoCon::spawn_command` then calls `CreateProcessW(..., EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT, ...)`
  for the shell. `child.process_id()` returns *only* the shell pid.

Because the host is a child of the daemon and portable-pty hides its pid, "assign the
shell's pid to a job" can never reach it.

### The fix: a daemon-wide reaping job (assign the daemon to the job, inherit everything)

`dflow_core::install_process_reaping_job()` (`crates/dflow-core/src/job.rs`) assigns the
**daemon's own process** to a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` at
startup, before any session can spawn (`crates/dflowd/src/main.rs`, right after the
single-instance lock is acquired).

The mechanism is process inheritance: Windows adds every new process to its parent's job
unless the child is created with `CREATE_BREAKAWAY_FROM_JOB`. Neither `CreatePseudoConsole`
(the host) nor `CreateProcessW` (the shell) passes that flag, so **both** the console host
and the shell - and every agent CLI and grandchild they spawn - land in the reaping job by
construction. When the daemon dies by any route (graceful `--stop`, panic, or a hard
`taskkill /F` of just the daemon pid), the OS closes the daemon's last handle to the job
and kill-on-close terminates every surviving member. No orphan can outlive the daemon.

The job handle is deliberately leaked for the process lifetime (`Job::into_handle`): closing
it while the daemon is alive would trip kill-on-close and take the daemon down immediately.
The OS closes it for us at process exit - exactly when the reaping should fire.

Per-session `KillGuard`s are kept for single-session teardown (`session.kill`): the shell,
already in the reaping job, nests into a per-session job on Windows 8+; if the OS refuses
the nesting the guard degrades to a direct child kill and the reaping job stays the
backstop. The two layers compose - the outer job guarantees nothing survives the daemon,
the inner jobs scope a single session's kill.

### Proof (two tests, both required to pass)

1. **Deterministic mechanism proof** -
   `crates/dflow-core/tests/reaping_membership.rs`. Installs the reaping job on the test
   process, spawns a real PTY session, and asserts the ConPTY console host appears in the
   job's member list (`QueryInformationJobObject(JobObjectBasicProcessIdList)`, surfaced as
   `dflow_core::reaping_job_console_host_pids()`). This does not depend on any Windows
   build's console-teardown behavior: membership in a `KILL_ON_JOB_CLOSE` job is the
   invariant that makes an orphan impossible. Red-green verified: with the install skipped
   the console host is absent from the member list and the test fails; with it, the host is
   a member and it passes. This is the regression guard - remove the install or let the host
   escape the job and it fails deterministically.

2. **End-to-end behavioral proof** - `crates/dflowd/tests/reaping.rs`. Starts a real
   `dflowd` under an isolated `DFLOW_DATA_DIR`, opens a session running a persistent
   `cmd /c ping -n 600` leaf, records the daemon's console-host + leaf descendants, then
   hard-kills the daemon process with `TerminateProcess` on just its pid (exactly
   `taskkill /F` without `/T` - no tree walk). It asserts every recorded process is gone
   within a couple seconds. Sample run: captured `[openconsole.exe, ping.exe]` under the
   daemon, all reaped after the hard kill.

### Platform note on the current dev machine

On Windows 11 build 26200, the OS also tears down console-*attached* children when a
pseudoconsole host exits, so an in-console tree is reaped even without our job on this
build. That OS behavior is build-dependent and was NOT reliable historically (hence the
leaks), which is why the deterministic membership test - not the kill-and-observe test -
is the mechanism's real guard. The reaping job makes reaping hold on every build,
including the ones where console teardown misbehaves.

## Deliverable 2 - dev vs prod ownership

`apps/desktop/src-tauri/src/daemon.rs`. Startup resolution is identical in both modes:
read the runtime file and, if it names a live daemon (pid alive AND the loopback socket
answers), connect (`started = false`). Only when there is no live daemon do the modes
diverge.

- **Mode selection** (`daemon_mode`): `DFLOW_DEV_EXTERNAL_DAEMON` is the explicit override
  (`1`/`true` -> dev-external, `0`/`false` -> prod-managed); unset defaults to dev-external
  for a debug build and prod-managed for a release build.
- **Production** (`prod-managed`): `ensure_managed_daemon` resolves the bundled daemon
  (sidecar next to the app exe, or `DFLOWD_PATH`, or - only in a dev checkout - the
  `target/` build), copies it into `%LOCALAPPDATA%/DapperFlow/bin/dflowd.exe` on first run
  and whenever the bundled version differs (compared via `dflowd --version`, added for this
  purpose), then spawns THAT copy fully detached (the existing `CREATE_BREAKAWAY_FROM_JOB` +
  `DETACHED_PROCESS` flags). It never runs the compiler's `target/` output in a packaged
  app, and never locks the file it may need to replace on update. A locked existing copy is
  tolerated (kept, not fatal).
- **Development** (`dev-external`): the app spawns NOTHING. If no daemon is live it returns
  `{ connected: false, mode: "dev-external", hint }`, and the connection UI (`DaemonBanner`)
  says to start the dev daemon (`just daemon-dev`) instead of spawning `target/debug`.
  Rebuilds never fight an exe lock.

The status bar distinguishes "attached" (connected to a running daemon) vs "started" (this
run spawned it) and shows a `dev daemon` tag in dev-external mode; Settings > Daemon shows
the full mode.

### Production bundling (packaging step, deliberately not in the default config)

The daemon must ship next to the app so `bundled_daemon_source` finds it. The Tauri
mechanism is `externalBin`, which places the sidecar next to the app executable. It is NOT
enabled in `tauri.conf.json` by default because Tauri validates it on every `cargo check`
AND `tauri build` (verified: both fail with "resource path ... doesn't exist" when the
sidecar is absent), which would break the standard dev/CI gates and the `--no-bundle`
verify unless a staging step ran first. `scripts/stage-daemon-sidecar.{ps1,sh}` builds
`dflowd` (release) and stages it as `binaries/dflowd-<triple>.exe`; a release pipeline runs
that, enables `externalBin`, and runs `pnpm tauri build`. The runtime resolution/copy logic
is complete and mode-agnostic; only the one-line config flip is left to the packager so the
default gates stay green.

## Deliverable 3 - dev control scripts

Repo-root `justfile` (with `[windows]`/`[unix]` recipes) delegating to standalone
`scripts/daemon-{dev,stop,status,restart}.{ps1,sh}` so the logic also works without `just`.
`daemon-dev` runs `cargo watch -x 'run -p dflowd'` when cargo-watch is installed and falls
back to a single `cargo run -p dflowd` with a note otherwise. Documented in CONTRIBUTING
under "Running the daemon in development". These replace the ad-hoc `taskkill /F` + rebuild
loop that caused the orphan leaks: every stop is a graceful `--stop`.

## Deliverables 4 & 5 - system tray, graceful quit, settings

`apps/desktop/src-tauri/src/{tray.rs,lib.rs}` plus the frontend store and Settings.

- **Tray** (Tauri 2 `TrayIconBuilder`): always present. Menu: Open DapperFlow (show/focus,
  or rebuild the window if it was closed), a live daemon status line (the frontend pushes
  "running · N sessions" via `set_tray_status`), Stop daemon, Restart daemon, Quit. Left
  click opens the window.
- **Close-to-tray**: the window's `CloseRequested` is intercepted and the window hidden, so
  closing the window never stops the daemon - the detached daemon and its agents keep
  running and the tray stays in control.
- **Graceful only**: tray Stop/Restart emit `tray://` events the frontend handles, reusing
  its confirm-when-live dialog (mounted app-wide) and the graceful WebSocket shutdown. Quit
  honors the keep-alive setting: ON leaves the daemon running; OFF sends a graceful
  `--stop` first (`daemon::graceful_stop` shells `dflowd --stop`). The app NEVER force-kills
  the daemon - force-kill stays safe only because of the reaping foundation, and is a human
  last resort.
- **Keep-alive setting** ("Keep agents running when I close the window", default ON):
  persisted to `%LOCALAPPDATA%/DapperFlow/app-settings.json` via `get/set_keep_alive`,
  surfaced as a toggle in Settings > Daemon alongside the mode, status, and Stop/Restart.

## Verification

- `cargo clippy --workspace --all-targets --locked -- -D warnings`: clean. Tauri app (a
  separate workspace) clippy: clean.
- `cargo test --workspace --lib --bins --locked`: 223 passed. `cargo test --workspace
  --test '*' --locked -- --test-threads=1`: all pass (both reaping tests green). Note:
  `dflow_everywhere` needs `target/debug/dflow.exe` present, so run the CI step `cargo build
  --workspace --all-targets` first (an isolated `--bins` run does not emit the runnable
  binary).
- `pnpm build` (tsc + vite) and `pnpm tauri build --debug --no-bundle`: green.

### Platform caveats

Windows-first. `install_process_reaping_job`, the per-session `KillGuard`, `pid_alive`, and
the detached-spawn flags are all `#[cfg(windows)]` with non-Windows stubs (reaping and
pid-liveness are no-ops that trust the socket probe; spawn is a plain detached child). Unix
process-group reaping (setsid + killpg / `PR_SET_PDEATHSIG`) lands when macOS and Linux
enter CI at M1, per `architecture.md`.
