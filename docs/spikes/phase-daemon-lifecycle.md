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
