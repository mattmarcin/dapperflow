#!/usr/bin/env bash
# Gracefully stop the running dflowd (marks sessions resumable and reaps the tree). Uses
# the built binary so a mid-edit broken build never blocks a stop.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

exe="$repo/target/debug/dflowd"
[ -f "$exe.exe" ] && exe="$exe.exe"
if [ ! -f "$exe" ]; then
    echo "dflowd not built yet; building once..."
    cargo build -p dflowd
    [ -f "$repo/target/debug/dflowd.exe" ] && exe="$repo/target/debug/dflowd.exe" || exe="$repo/target/debug/dflowd"
fi
exec "$exe" --stop
