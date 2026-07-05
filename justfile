# DapperFlow developer tasks. Run `just <task>`.
#
# The daemon tasks own the DEV daemon lifecycle so you never `taskkill /F` + rebuild by
# hand (the habit that used to orphan ConPTY hosts). In development the desktop app does
# NOT spawn the daemon - it connects to the one these tasks run - so rebuilds never fight
# an exe lock. See CONTRIBUTING.md "Running the daemon in development".
#
# Each recipe delegates to a standalone script in scripts/ so the logic also works without
# `just` installed (PowerShell on Windows, bash elsewhere).

# List available tasks.
default:
    @just --list

# Run the dev daemon under a file watcher (auto-rebuild + restart on any crate change).
[windows]
daemon-dev:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/daemon-dev.ps1
[unix]
daemon-dev:
    bash scripts/daemon-dev.sh

# Gracefully stop the running daemon (dflowd --stop: reaps the tree, marks sessions resumable).
[windows]
daemon-stop:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/daemon-stop.ps1
[unix]
daemon-stop:
    bash scripts/daemon-stop.sh

# One-line daemon status (running, pid, port, live session count).
[windows]
daemon-status:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/daemon-status.ps1
[unix]
daemon-status:
    bash scripts/daemon-status.sh

# Stop, rebuild, and start the daemon detached (a clean restart without the watcher).
[windows]
daemon-restart:
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts/daemon-restart.ps1
[unix]
daemon-restart:
    bash scripts/daemon-restart.sh
