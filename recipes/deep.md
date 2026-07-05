---
name: deep
version: 1
description: Full Plan Studio loop with as many annotation rounds as needed, explicit approval, staged implementation, adversarial review.
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

Design matters here, so invest in the artifact.
Surface the real decisions and their tradeoffs, and expect more than one annotation round.
Hold the plan review loop: open the artifact, then wait on a foreground `dflow plan poll`, revising `plan.html` in place and re-opening it between rounds.
Do not begin implementation until a poll returns `approved`; your planning session is not done before then, so never background the poll or end the session while it is your turn to wait.

## implement

Implement in stages that map to the approved plan, committing at each meaningful boundary.
When a stage surfaces a decision the plan did not settle, stop and ask through `dflow status blocked "<why>"`.

## verify

The project's registered check commands run in a gate-class worktree (checks-only in this milestone; adversarial cross-model review lands with the gate engine).
Make the checks pass before you report done.

## ship

Delivery is a pull request, and the push waits for an explicit human approval.
Write a PR body that states what changed and why, references the card, and links the approved plan.
