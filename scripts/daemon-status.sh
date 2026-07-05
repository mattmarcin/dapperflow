#!/usr/bin/env bash
# One-line dflowd status (running, pid, port, live session count, data dir).
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
exec "$exe" --status
