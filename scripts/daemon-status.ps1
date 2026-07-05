# One-line dflowd status (running, pid, port, live session count, data dir).
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$exe = Join-Path $repo 'target\debug\dflowd.exe'
if (-not (Test-Path $exe)) {
    Write-Host 'dflowd not built yet; building once...' -ForegroundColor Yellow
    cargo build -p dflowd
}
& $exe --status
exit $LASTEXITCODE
