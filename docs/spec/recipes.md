# Flow Recipes Specification

Flow recipes make DapperFlow's opinions optional.
The default workflow (worktree isolation, artifact plans, verification gate) is one way to build software; a recipe is a shareable markdown file that changes how orchestration works, per card, per project, or globally.
Recipes are to orchestration what skills are to agents: structured front matter the engine enforces, natural-language guidance the agents follow.

## Goals

1. Users can weaken or strengthen any opinion: skip HTML planning, add extra review stages, force a specific harness, mount outside MCP servers, change gate strictness.
2. Recipes are single markdown files: easy to write, diff, commit, and share (a URL or file is an install).
3. The built-in dials (Presto, Standard, Deep) are themselves bundled recipes with no special powers, proving the layer is real.
4. The bundled set also includes `audit` and `audit-deep` (investigation-shaped, no ship stage; see product.md "Card sources: onboarding audit").

An optional `budgets:` front-matter block (`cards:`, `notes:`) is engine-enforced: a session dispatched under the recipe cannot exceed those creation counts; overflow returns a structured error telling the agent to put the remainder in its report.

## Format

A recipe is markdown with YAML front matter.
Front matter is machine-enforced by the engine; the body is stage-tagged natural-language guidance injected into agent briefs.

```markdown
---
name: standard
version: 1
description: One-screen plan artifact, single review round, full gate.
extends: base                     # optional inheritance; overrides merge shallowly
stages: [plan, implement, verify, ship]

plan:
  mode: artifact                  # artifact | markdown | none
  approval: required              # required | auto
  playbooks: [plan]

implement:
  harness: default                # default | claude | codex | opencode | pi
  model: default
  effort: default
  worktree: pooled                # pooled | fresh | in_place (in_place requires explicit ack)

mcp:                              # extra MCP servers mounted into sessions for this recipe
  - name: context7
    command: "npx -y @upstash/context7-mcp"
    stages: [plan, implement]

verify:
  gate: full                      # full | checks_only | none
  reviewer_harness: different     # different | any concrete adapter name

ship:
  target: pr                      # pr | local_merge | none
---

## plan

Keep the artifact to one screen.
Lead with the two or three decisions you actually need from the human; use controls for them.

## implement

Work in small commits.
Run `dflow status blocked` with a concrete question rather than guessing on product decisions.
```

## Resolution and scoping

- Scopes: bundled (shipped with the app) -> user (`<app-data>/recipes/`) -> project (`<project>/.dapperflow/recipes/`).
- A card's dial selects a recipe by name; resolution order is card selection > project `default_recipe` > global default (`standard`).
- Name collisions resolve most-specific-scope-first; the UI always shows which file won.
- `extends` allows a project recipe to tweak one knob of a bundled recipe without copying it.

## What recipes control (v1 surface)

- Stage list and order; whether planning happens and in what mode; approval requirements.
- Harness/model/effort per stage; worktree strategy.
- Extra MCP servers mounted into that recipe's sessions (the escape hatch for users who want outside tools in their flows).
- Gate strictness, reviewer harness, ship target.
- Brief guidance text per stage.

Out of scope for v1 (explicitly deferred): arbitrary user-defined stages with custom engine semantics, recipe-defined UI panels, and turing-complete hooks.
The stage vocabulary is fixed (`shape, plan, implement, verify, ship`); recipes choose among fixed behaviors per stage plus free-text guidance.
This keeps recipes safe to share and possible to validate.

## Validation and safety

- `recipe.validate` (also run on install and on file change) parses front matter against the schema and reports precise errors; invalid recipes never partially apply.
- Validation also checks recipe x harness compatibility: a recipe that mounts MCP servers fails validation for a dispatch onto a harness without verified MCP support (capability matrix in adapters.md), at dispatch time, not mid-run.
- Recipes are classified into trust tiers per `security.md / Recipe trust tiers`: **standard** recipes run with no extra consent; **privileged** recipes (any of `mcp` mounts, `worktree: in_place`, `verify.gate: none` when a ship stage is present, `ship.target: local_merge`) require an explicit per-project grant that lists exactly what is elevated and is re-confirmed when the recipe file's hash changes. A shipless recipe (like `audit`) with `gate: none` is standard: there is nothing to gate.
- Recipes are inert text; nothing in a recipe executes at install time.

## Sharing

- `recipe.install { source: path|url, scope }` copies the file and validates; a recipe gallery is a later, separate concern.
- Sharing risk, stated honestly: a standard recipe still shapes agent behavior through injected guidance (prompt-injection surface), and a privileged recipe carries real execution and delivery risk (MCP commands, in-place edits, gate bypass).
  The trust-tier grants above are the control; "it is just markdown" is deliberately not the safety argument.

## Engine integration

- Dispatch resolves the recipe first; everything downstream (worktree strategy, env materialization, brief composition, session creation, gate configuration, ship behavior) reads recipe output, not hardcoded policy.
- The recipe name and version are recorded on every dispatch in `card_events`, so timelines show which flow produced which outcome.
