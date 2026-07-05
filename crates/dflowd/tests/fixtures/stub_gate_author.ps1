# Scripted stub "author" for the gate e2e (gate.md). No real LLM: it writes a small
# source file carrying a seeded off-by-one bug (an intent-touching problem the reviewer
# must catch) plus trailing whitespace (a safe-mechanical problem the fixer autofixes),
# then commits it in its dispatch worktree. The gate later checks out this commit.
$ErrorActionPreference = "Continue"
$lines = @(
  "function run() {",
  "  for (i = 0; i <= items.length; i++) {   ",
  "    process(items[i])",
  "  }",
  "}"
)
Set-Content -Path "feature.txt" -Value $lines
& git add -A 2>&1 | Out-Null
& git commit -m "author: feature with seeded off-by-one bug" 2>&1 | Out-Null
Set-Content -Path "author.log" -Value "AUTHOR committed"
