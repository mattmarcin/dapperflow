#!/usr/bin/env bash
# Run dflowd for development under a file watcher.
#
# The desktop app in dev mode (DFLOW_DEV_EXTERNAL_DAEMON=1, the default for a debug build)
# connects to THIS externally-owned daemon instead of spawning target/debug, so app
# rebuilds never fight an exe lock. cargo-watch auto-rebuilds and restarts on any change.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

if command -v cargo-watch >/dev/null 2>&1; then
    echo "Watching crates; the daemon auto-restarts on change. Ctrl-C to stop."
    exec cargo watch -x 'run -p dflowd'
else
    echo "cargo-watch not found - running the daemon once without auto-restart."
    echo "Install the watcher for auto-rebuild: cargo install cargo-watch"
    exec cargo run -p dflowd
fi
