---
name: presto
version: 1
description: Dispatch immediately from a short brief, run checks, open a PR. For obvious fixes.
stages: [implement, verify, ship]

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

## implement

You have a short brief and no planning stage.
Confirm your understanding of the ask in one line, then start.
Work in small commits with clear messages.
Run `dflow status blocked "<why>"` with a concrete question rather than guessing on a product decision.
Call `dflow status done` when the change is complete and the checks below would pass.

## verify

The project's registered check commands run in a gate-class worktree (checks-only in this milestone; adversarial review lands with the gate engine).
Make the checks pass before you report done; a red check is not a finished task.

## ship

Delivery is a pull request, and the push waits for an explicit human approval.
Write a PR body that states what changed and why, and reference the card.
