# Scripted stub "planning agent" for the Plan Studio loop (plan-studio.md, agent-cli.md).
#
# No real LLM: this drives the actual `dflow` CLI end to end exactly as a worker agent
# would. Dispatch injects DFLOW_TOKEN / DFLOW_CARD / DFLOW_ENDPOINT and puts `dflow` on
# PATH; this script writes a self-contained plan.html, opens it for review, then loops on
# `dflow plan poll`, revising the artifact in place and re-opening between rounds until the
# human approves (or ends) the review. Every `dflow` invocation and its output is logged
# so the driving test can prove the whole loop ran over the real CLI.
param([string]$WorkDir = ".")

$ErrorActionPreference = "Continue"
Set-Location -Path $WorkDir
$plan = Join-Path $WorkDir "plan.html"
$log = Join-Path $WorkDir "agent.log"

function Log($m) { Add-Content -Path $log -Value $m }

$round = 1
$html = "<!doctype html><html><head><meta charset=utf-8><title>Retry plan</title></head>" +
        "<body><h1>Retry plan (v$round)</h1><p id=retry>retry with exponential backoff</p>" +
        "<form><label>Storage <select name=storage><option>sqlite</option><option>postgres</option></select></label></form>" +
        "</body></html>"
Set-Content -Path $plan -Value $html
Log "AGENT wrote plan round $round"

$open = & dflow plan open $plan 2>&1 | Out-String
Log "AGENT open: $open"

for ($i = 0; $i -lt 8; $i++) {
    $poll = & dflow plan poll 2>&1 | Out-String
    Log "AGENT poll: $poll"
    if ($poll -match "review approved" -or $poll -match "review ended") {
        Log "AGENT done after $round round(s)"
        break
    }
    $round++
    Add-Content -Path $plan -Value "<!-- revised for round $round per the human's feedback -->"
    $reopen = & dflow plan open $plan 2>&1 | Out-String
    Log "AGENT reopen round ${round}: $reopen"
}

Log "AGENT exiting"
