---
name: standard
version: 1
description: One-screen plan artifact, single review round, checks, PR.
stages: [plan, implement, verify, ship]

plan:
  mode: artifact
  approval: required
  playbooks: [plan]

implement:
  harness: default
  model: default
  effort: default
  worktree: pooled

verify:
  gate: checks_only
  reviewer_harness: different

ship:
  target: pr
---

## plan

Keep the artifact to one screen.
Lead with the two or three decisions you actually need from the human, and use controls for them.
Do not over-specify; the point is a fast skim-and-approve, not a design document.
After you open it, hold a foreground `dflow plan poll` until it returns `approved` - one round is usually enough, but do not end your session before approval.

## implement

Work in small commits.
Follow the approved plan; if reality diverges from it, say so in a status note rather than silently improvising.
Run `dflow status blocked "<why>"` with a concrete question rather than guessing on a product decision.

## verify

The project's registered check commands run in a gate-class worktree (checks-only in this milestone).
Make the checks pass before you report done.

## ship

Delivery is a pull request, and the push waits for an explicit human approval.
Write a PR body that states what changed and why, and reference the card.
