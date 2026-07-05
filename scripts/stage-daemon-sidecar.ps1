# Stage the dflowd sidecar for a PRODUCTION Tauri bundle.
#
# Builds dflowd (release) and copies it to apps/desktop/src-tauri/binaries/ with the
# Tauri sidecar naming (dflowd-<target-triple>.exe). To ship a real installer that carries
# the daemon, run this, then add "externalBin": ["binaries/dflowd"] under `bundle` in
# apps/desktop/src-tauri/tauri.conf.json, then `pnpm tauri build`. Tauri places the sidecar
# next to the app executable, where the app resolves and copies it to the stable managed
# location at first run (see apps/desktop/src-tauri/src/daemon.rs).
#
# externalBin is intentionally NOT enabled by default: it is validated on every `cargo
# check` and `tauri build`, so leaving it off keeps the standard dev/CI gates green without
# a mandatory staging step. This script is the release-time bridge.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

Write-Host 'Building dflowd (release)...' -ForegroundColor Cyan
cargo build --release -p dflowd

$triple = (& rustc -vV | Select-String '^host:').ToString().Split(' ')[1]
$src = Join-Path $repo 'target\release\dflowd.exe'
$destDir = Join-Path $repo 'apps\desktop\src-tauri\binaries'
$dest = Join-Path $destDir "dflowd-$triple.exe"

New-Item -ItemType Directory -Force -Path $destDir | Out-Null
Copy-Item -Force $src $dest
Write-Host "Staged sidecar: $dest" -ForegroundColor Green
Write-Host 'Now enable externalBin in tauri.conf.json and run: pnpm tauri build' -ForegroundColor Green
