# Stop, rebuild, and start dflowd detached - a clean restart without the file watcher.
# Prefer `just daemon-dev` for day-to-day work; this is for a one-off fresh daemon.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# 1. Graceful stop (ignored if nothing is running).
& (Join-Path $PSScriptRoot 'daemon-stop.ps1')

# 2. Rebuild.
Write-Host 'Rebuilding dflowd...' -ForegroundColor Cyan
cargo build -p dflowd

# 3. Start detached so the terminal is free and the daemon outlives this shell.
$exe = Join-Path $repo 'target\debug\dflowd.exe'
Start-Process -FilePath $exe -WindowStyle Hidden
Write-Host 'dflowd restarted (detached). Check it with: just daemon-status' -ForegroundColor Green
