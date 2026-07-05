# Scripted stub "fixer" that makes NO change (gate.md / Autofix earned-claim). No real
# LLM: it simply exits without touching the worktree, emulating a fixer session that was
# killed/aborted before committing, or one that decided (wrongly) it had nothing to do.
# The mechanical finding's own class (trailing whitespace) does NOT fail the project
# check, so the re-check stays green even though the defect remains - which is exactly the
# false-pass trap. With the earned-claim fix the gate must escalate, never autofix.
# It writes no marker file so the worktree stays byte-for-byte as the reviewer left it.
$ErrorActionPreference = "Continue"
exit 0
