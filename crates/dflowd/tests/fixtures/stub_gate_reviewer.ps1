# Scripted stub "adversarial reviewer" for the gate e2e (gate.md / Adversarial review).
# No real LLM: it drives the real `dflow finding add` CLI exactly as a reviewer agent on a
# DIFFERENT harness than the author would - filing one safe-mechanical finding (autofixed)
# and one intent-touching finding (the seeded bug, which escalates to a Needs You). The
# per-task token injected by the gate carries the gate run id, so the daemon routes the
# findings to the right run. Then it exits.
$ErrorActionPreference = "Continue"
$r1 = & dflow finding add --severity minor --category mechanical --body "trailing whitespace after the for-loop brace in feature.txt" 2>&1 | Out-String
$r2 = & dflow finding add --severity major --category intent --body "off-by-one: the loop bound uses <= items.length and reads one past the end" 2>&1 | Out-String
Set-Content -Path "reviewer.log" -Value ("R1:`n" + $r1 + "R2:`n" + $r2)
