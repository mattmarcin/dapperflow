#!/usr/bin/env bash
# Stage the dflowd sidecar for a PRODUCTION Tauri bundle.
#
# Builds dflowd (release) and copies it to apps/desktop/src-tauri/binaries/ with the Tauri
# sidecar naming (dflowd-<target-triple>[.exe]). To ship a real installer that carries the
# daemon, run this, then add "externalBin": ["binaries/dflowd"] under `bundle` in
# apps/desktop/src-tauri/tauri.conf.json, then `pnpm tauri build`. Tauri places the sidecar
# next to the app executable, where the app resolves and copies it to the stable managed
# location at first run (see apps/desktop/src-tauri/src/daemon.rs).
#
# externalBin is intentionally NOT enabled by default: it is validated on every `cargo
# check` and `tauri build`, so leaving it off keeps the standard dev/CI gates green without
# a mandatory staging step. This script is the release-time bridge.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

echo "Building dflowd (release)..."
cargo build --release -p dflowd

triple="$(rustc -vV | sed -n 's/^host: //p')"
ext=""
[ -f "target/release/dflowd.exe" ] && ext=".exe"
src="target/release/dflowd$ext"
dest_dir="apps/desktop/src-tauri/binaries"
dest="$dest_dir/dflowd-$triple$ext"

mkdir -p "$dest_dir"
cp -f "$src" "$dest"
echo "Staged sidecar: $dest"
echo "Now enable externalBin in tauri.conf.json and run: pnpm tauri build"
