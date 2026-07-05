# Daemon Lifecycle Specification

How `dflowd` is spawned, owned, reaped, and controlled, in development and in production.
Added 2026-07-05 after repeated pain: the app locking the daemon binary during dev rebuilds, and force-kills orphaning ConPTY console hosts.

## Correctness foundation: total process reaping

Every process the daemon spawns must die with the daemon, however the daemon dies (graceful `--stop`, crash, or `taskkill /F`).

- Each session's PTY child AND its ConPTY host (`OpenConsole.exe`) are assigned to the daemon's Windows Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` at spawn time.
- portable-pty's ConPTY backend launches `OpenConsole.exe` as a child; that host, not just the shell, must be in the job. Verify explicitly (this is the gap `docs/debt`/prior notes recorded: the shell was in the job but the console host leaked).
- Acceptance: spawn a session, then end the daemon three ways (`--stop`, a hard kill, a panic); assert zero surviving `OpenConsole.exe`/`conhost.exe` whose parent was that daemon. No orphan sweeps should ever be needed again.

Once this holds, killing the daemon is always safe, and every other decision below gets simpler.

## Ownership: development vs production

The app must never lock the binary it might need to rebuild.

**Production**: the app bundles `dflowd` and runs it from a stable, writable location (for example `%LOCALAPPDATA%/DapperFlow/bin/dflowd.exe`), copied from the app bundle on first run and whenever the bundled version is newer.
It never runs the compiler's `target/` output.
Spawn is detached (survives the app), single-instance via the lock + runtime file.

**Development**: the app does NOT spawn the daemon.
A dev workflow runs the daemon externally under a file watcher (`cargo watch -x 'run -p dflowd'`) that auto-rebuilds and restarts on any crate change and writes the runtime file; the app connects as a pure client.
Selected by a dev signal (`DFLOW_DEV_EXTERNAL_DAEMON=1`, or dev-build default): the app tries the runtime file first and connects to whatever live daemon it finds; if none, it shows "start the dev daemon (`just daemon-dev`)" instead of spawning `target/debug`.
Result: rebuilds never fight an exe lock, and never orphan anything.

## Startup resolution (both modes)

1. Read the runtime file (port + token + pid).
2. If it names a live daemon (pid alive, socket answers), connect. Status bar says "connected to running daemon".
3. Else, stale runtime file: reclaim the lock (pid-alive check) and, in production, spawn the bundled daemon; in dev, prompt to start the dev daemon.
4. A crashed daemon never blocks the next start; the lock and runtime file are reclaimed by pid-liveness, never by manual file deletion.

## Control surface

- `dflowd --stop` (graceful: reaps the tree via the Job Object, flushes scrollback, marks sessions `interrupted` and resumable) and `dflowd --status` (one line: running, pid, port, session count) already exist and are the canonical controls.
- Repo dev scripts (justfile or `scripts/`): `daemon-dev` (cargo-watch run), `daemon-stop`, `daemon-status`, `daemon-restart` (stop -> rebuild -> start). These replace ad-hoc `taskkill /F` + rebuild, which was the source of the orphan leaks. Documented in CONTRIBUTING.

## Quit behavior (user decision 2026-07-05: keep running, show in tray)

- Quitting the app window does NOT stop the daemon by default: it stays detached and its agents keep working ("the GUI is a lens").
- A **system tray** presence makes a backgrounded daemon visible and controllable: it shows status (running/stopped, live session count) and offers Open (focus or relaunch the window), Stop daemon (graceful `--stop`, with a confirm when live sessions exist), and Restart daemon.
- Setting "Keep agents running when I close the window" (default ON). When OFF, quitting sends the graceful `--stop`, so nothing lingers and the next launch offers one-click resume of the interrupted sessions.
- Every app-initiated shutdown is graceful (`daemon.shutdown`/`--stop`), never a force-kill. Force-kill stays safe only because of the reaping foundation above, and is a human last resort, not an app path.

## Non-goals

- The daemon is not a Windows Service / launchd / systemd unit in v1; it is a user-session process managed by the app + tray. OS-service installation is a later, opt-in concern.
