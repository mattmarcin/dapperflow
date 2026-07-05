# Run dflowd for development under a file watcher.
#
# The desktop app in dev mode (DFLOW_DEV_EXTERNAL_DAEMON=1, the default for a debug build)
# connects to THIS externally-owned daemon instead of spawning target/debug, so app
# rebuilds never fight an exe lock and never orphan anything. cargo-watch auto-rebuilds and
# restarts the daemon on any crate change; each restart's graceful exit reaps its tree via
# the Job Object, so there is nothing to clean up.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

if (Get-Command cargo-watch -ErrorAction SilentlyContinue) {
    Write-Host 'Watching crates; the daemon auto-restarts on change. Ctrl-C to stop.' -ForegroundColor Cyan
    cargo watch -x 'run -p dflowd'
} else {
    Write-Host 'cargo-watch not found - running the daemon once without auto-restart.' -ForegroundColor Yellow
    Write-Host 'Install the watcher for auto-rebuild: cargo install cargo-watch' -ForegroundColor Yellow
    cargo run -p dflowd
}
