# Scripted stub "fixer" that makes a REAL change but leaves it UNCOMMITTED (gate.md /
# Autofix earned-claim criterion (b): "a working-tree diff the gate then commits,
# attributable to the fixer"). No real LLM: it strips the trailing whitespace from
# feature.txt (the safe-mechanical fix) but does not run git commit, so the gate must
# detect the working-tree diff, commit it attributably, record the commit, and only then
# mark the mechanical finding autofixed.
$ErrorActionPreference = "Continue"
$trimmed = Get-Content -Path "feature.txt" | ForEach-Object { $_.TrimEnd() }
Set-Content -Path "feature.txt" -Value $trimmed
