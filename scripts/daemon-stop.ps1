# Gracefully stop the running dflowd (marks sessions interrupted/resumable and reaps the
# whole process tree via the Job Object). Uses the built binary so a mid-edit broken build
# never blocks a stop; builds it only if it is missing.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

$exe = Join-Path $repo 'target\debug\dflowd.exe'
if (-not (Test-Path $exe)) {
    Write-Host 'dflowd not built yet; building once...' -ForegroundColor Yellow
    cargo build -p dflowd
}
& $exe --stop
exit $LASTEXITCODE
