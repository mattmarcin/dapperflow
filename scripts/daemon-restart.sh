#!/usr/bin/env bash
# Stop, rebuild, and start dflowd detached - a clean restart without the file watcher.
# Prefer `just daemon-dev` for day-to-day work; this is for a one-off fresh daemon.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

# 1. Graceful stop (ignored if nothing is running).
bash "$here/daemon-stop.sh" || true

# 2. Rebuild.
echo "Rebuilding dflowd..."
cargo build -p dflowd

# 3. Start detached so the terminal is free and the daemon outlives this shell.
exe="$repo/target/debug/dflowd"
[ -f "$exe.exe" ] && exe="$exe.exe"
nohup "$exe" >/dev/null 2>&1 &
disown || true
echo "dflowd restarted (detached). Check it with: just daemon-status"
