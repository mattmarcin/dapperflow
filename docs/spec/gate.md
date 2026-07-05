# Verification Gate Specification

Nothing becomes a PR because an agent says it is done; branches earn their way out through the gate.
A git-proxy-style validation pipeline, implemented natively in the daemon and configured per project and per recipe.

## Pipeline

Runs in an isolated worktree leased from the project's pool under the `gate` lease class (never the authoring worktree, never the user's checkout; warm caches intact because that is exactly what checks need; env materialized in checks-only mode), triggered when a card's implement stage reports done or when the user clicks Verify.

1. **Checks**: the project's registered check commands (build, test, lint, typecheck) run in order; output captured as evidence. `projects.check_cmds` may have been seeded by the onboarding audit's confirmed proposal (provenance note; behavior unchanged).
2. **Adversarial review**: a reviewer session on a different harness than the author (recipe `reviewer_harness: different` is the default) receives the diff, the card's acceptance criteria, and the plan artifact if one exists; it produces structured findings via `dflow finding add`.
3. **Autofix**: findings classified safe-mechanical (lint, formatting, dead imports, trivial test fixes) are handed to a fixer session, then re-checked; a finding is marked `autofixed` only when the autofix earns the claim (see **Autofix earned claim** below), otherwise it escalates like an intent finding.
4. **Escalation**: findings touching intent (behavior, API shape, scope) become Needs You items; the finding review renders in Plan Studio chrome so the human resolves each with approve / fix / skip, annotated in place.
5. **Ship**: only when every check is green and every finding resolved does the branch push and the PR open; CI status streams back onto the card; CI failures can trigger one bounded autofix loop before escalating.

Recipe knobs: `gate: full | checks_only | none`, reviewer harness, autofix aggressiveness.
Project modes: `pr` (default) or `local_only` (gate ends with an approved local fast-forward merge instead of a push).

## Autofix earned claim

A mechanical finding may be marked `autofixed` only when ALL of the following hold; a green re-check alone is never sufficient.

1. The fixer session **completed** - it exited on its own, not killed at the session timeout.
2. The fixer **actually changed the worktree** - either its own new commit advanced HEAD, or it left an uncommitted working-tree diff that the gate then commits, attributable to the fixer.
3. The **re-check is green** after that change.

This is required because the exact defect class autofix targets (lint, formatting, dead imports, trivial test fixes) does not fail the project check by construction.
A fixer that was killed before committing, or that made no change, therefore leaves the defect in the diff while the re-check passes; marking such a finding `autofixed` would be a false pass (the product lying about the very defect class autofix exists to handle).

When any of the three does not hold, the mechanical finding is **not** autofixed: it stays open and escalates to a `gate_finding` Needs You, exactly like an intent finding.
The autofix step records the reason as evidence, alongside the fixer's tail output:

- the fixer was killed or timed out -> reason `fixer did not complete`;
- the fixer made no change -> reason `autofix made no changes`;
- the fixer changed code but the re-check failed -> reason `re-check failed after autofix`.

On a successful autofix the evidence records the fixer's commit id and a diffstat (and whether the gate committed a working-tree diff the fixer left uncommitted), so the timeline shows exactly what the fixer changed - never a prose-only claim.

## Findings

- Severity: blocker, major, minor; every finding needs a concrete failure scenario or rule citation, not vibes.
- All findings and resolutions are `card_events` with evidence pointers; the timeline shows exactly why a branch was allowed out.

## GitHub integration

- Two transports with an explicit boundary (revised 2026-07-04, user decision): **push** goes through the system git CLI using the user's existing credential helper (the same path their manual pushes use); **API operations** (PR create with generated summary linking the card, CI check watching via `gh pr checks`, merge with squash default, PR-head containment checks, issue import per product.md) go through the **`gh` CLI with `--json` output** - gh-first because its users are already authenticated, token storage is the OS credential manager's, and it removes an entire OAuth-app registration and device-flow pairing UX from M5.
- `gh` is a feature-scoped dependency per product principles: detected via `gh auth status`; absent or unauthenticated, PR mode degrades cleanly to local-only with a one-line setup pointer. The direct-API path (octocrab + device-flow OAuth) is explicitly deferred unless gh-less demand materializes.
- When the card's origin is a GitHub issue, the generated PR body includes `Fixes #<n>` so the issue closes on merge; the PR summary links both the card and the issue.

## Teardown safety

- A worktree returns to the pool only when its work is provably landed: HEAD reachable from a remote branch, PR merged and head contained, or explicit user discard with a typed confirmation.
- Dirty or ambiguous worktrees park as `dirty` with a Needs You item; nothing is ever auto-deleted.
