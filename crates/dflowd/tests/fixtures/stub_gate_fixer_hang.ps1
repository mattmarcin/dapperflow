# Scripted stub "fixer" that never completes (gate.md / Autofix earned-claim criterion
# (a): the fixer session must run to completion, not be killed at the session timeout).
# No real LLM: it sleeps far past the gate session timeout so the gate kills it, then the
# gate must escalate with reason "fixer did not complete" and never mark anything
# autofixed. It changes nothing in the worktree.
$ErrorActionPreference = "Continue"
Start-Sleep -Seconds 120
