# Reviewer stub that files ONLY a safe-mechanical finding, so the gate autofixes it and
# then PASSES with no open findings - proving the full checks -> review -> autofix -> pass
# path (gate.md / Pipeline). No real LLM.
$ErrorActionPreference = "Continue"
$r = & dflow finding add --severity minor --category mechanical --body "trailing whitespace after the for-loop brace in feature.txt" 2>&1 | Out-String
Set-Content -Path "reviewer.log" -Value ("MECH:`n" + $r)
