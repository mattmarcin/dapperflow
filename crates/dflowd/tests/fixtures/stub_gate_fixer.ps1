# Scripted stub "fixer" for the gate e2e (gate.md / Autofix). No real LLM: it applies the
# safe-mechanical fix (strip trailing whitespace) and commits, leaving the intent-touching
# bug for the human. The gate re-runs the checks after this and, if they stay green, marks
# the mechanical finding autofixed. Then it exits.
$ErrorActionPreference = "Continue"
$trimmed = Get-Content -Path "feature.txt" | ForEach-Object { $_.TrimEnd() }
Set-Content -Path "feature.txt" -Value $trimmed
& git add -A 2>&1 | Out-Null
& git commit -m "fixer: strip trailing whitespace" 2>&1 | Out-Null
Set-Content -Path "fixer.log" -Value "FIXER committed"
