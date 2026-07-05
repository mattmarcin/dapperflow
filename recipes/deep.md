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
Do not begin implementation until the plan is explicitly approved.

Note for this milestone: the Plan Studio review loop and enforced plan approval arrive with Plan Studio.
Until then this stage validates and parses, but the daemon cannot yet hold you at an unapproved plan; treat the plan as guidance and keep the human in the loop through status notes.

## implement

Implement in stages that map to the approved plan, committing at each meaningful boundary.
When a stage surfaces a decision the plan did not settle, stop and ask through `dflow status blocked "<why>"`.

## verify

The project's registered check commands run in a gate-class worktree (checks-only in this milestone; adversarial cross-model review lands with the gate engine).
Make the checks pass before you report done.

## ship

Delivery is a pull request, and the push waits for an explicit human approval.
Write a PR body that states what changed and why, references the card, and links the approved plan.
