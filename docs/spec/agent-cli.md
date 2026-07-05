# Agent CLI Specification (`dflow`)

The agent-side contract: a tiny cross-platform binary placed on PATH in every leased worktree.
It is how worker agents (on any harness) report state, read their brief, run the plan-review loop, and file findings.
Chosen over MCP for workers deliberately: a CLI works identically in every harness with zero mount configuration, and its output cost is fully under our control.

Named `dflow`, not `df`, to avoid colliding with the Unix disk-free command.

## Design rules (AXI principles, applied)

1. Token-minimal output: compact aligned tables, 3-4 fields, no JSON unless `--json` is passed.
2. Pre-computed aggregates: counts and statuses included so agents never issue follow-up list calls.
3. Definitive empty states: `no feedback queued` rather than empty output.
4. Structured errors on stderr with stable exit codes; never an interactive prompt; mutations idempotent.
5. Content first: bare `dflow` prints the agent's current card, state, and next expected action.
6. Every response ends with a `next:` line telling the agent its most likely next step.
7. Large payloads are truncated with size hints and a `--full` escape hatch.

## Authentication and wiring

- Dispatch injects `DFLOW_TOKEN` (per-task scoped), `DFLOW_CARD`, and the daemon WS endpoint into the session environment.
- The binary talks `dflow-proto` over the same WS the desktop uses; scope limits it to its own card, session, and artifacts.
- Outside a dispatched worktree it fails fast with a one-line explanation and exit code 3.

## Verbs

| Verb | Purpose | Notes |
|---|---|---|
| `dflow` | Current card, state, next action | Content-first default |
| `dflow card` | Brief, acceptance criteria, project memory digest | `--full` for complete brief |
| `dflow status <working\|blocked\|done> [note]` | Tier-1 lifecycle self-report | `blocked` requires a note; `done` is a stage-advance request, arbitrated by the recipe (below) |
| `dflow card create --title <t> [--type <t>] [--brief -] [--fingerprint <slug>]` | File a new card from within a session | Session-first: agents maintain the board as a side effect of conversation; a session may create many cards. `--fingerprint` sets a stable dedupe slug (audit runs stamp `origin: audit` automatically); exceeding a recipe card budget returns a structured error whose `next:` line says to put the remainder in the report. Audit-scoped tokens cannot move lanes on created cards |
| `dflow card update [<id>] [--title] [--brief -]` / `dflow card note <text>` | Update a card / set the session-strip status note | Defaults to the session's own card; `note` powers the board's live status line |
| `dflow card move [<id>] <lane>` | Move a card on the board | Recipe-gated lanes (verifying, pr, done) are arbitrated like `status done` |
| `dflow know [find\|get\|add]` | Project knowledgebase verbs | See knowledge.md; autonomous writes, evidenced by `knowledge_updated` events |
| `dflow plan open <file.html>` | Register a plan artifact for review | Idempotent per path; returns review URL for the human |
| `dflow plan poll` | Bounded poll for human feedback | Blocks up to ~4 minutes (under harness tool timeouts), then returns feedback items, layout warnings, `pending` + re-poll guidance, or `ended` + `next_step`; safe to re-run forever, feedback is never lost |
| `dflow finding add --severity <s> --body <text>` | File a finding (gate, scout reports) | |
| `dflow help [verb]` | Concise per-verb reference | |

## Stage advancement arbitration

`dflow status done` never advances a stage by itself; it is a request.
The daemon checks the recipe's conditions for the current stage: if they are satisfied (for example the plan stage with `approval: auto`, or an implement stage with no pending gate), the stage advances and the response tells the agent what is next.
If a required condition is unmet (plan `approval: required` without a recorded `plan_approved`), the response says exactly what is missing, the session moves to `awaiting_feedback` or `idle` per stage, and a Needs You item is raised for the human.
Agent signals are inputs; recipe conditions are gates; the daemon is the only party that transitions stages.

## Example outputs

```
$ dflow status blocked "need a decision: soft-delete vs hard-delete for accounts"
recorded: blocked
next: stop working; the captain has been notified and will respond via steer or plan feedback
```

```
$ dflow plan poll
feedback (2 items, round 3):
  1 [text-range] "retry with exponential backoff" > user: cap at 3 attempts, then dead-letter
  2 [control q:storage] user selected: sqlite
layout: clean
next: revise the artifact in place, then run `dflow plan poll` again
```

```
$ dflow card
card 01JX...  feature  "Add dark mode toggle"  dial:standard  project:acme-web
acceptance (3):
  1 toggle persists across sessions
  2 respects system preference by default
  3 no flash of wrong theme on load
memory digest: acme-web uses tailwind + next-themes is NOT installed; check docs/theming.md
next: run `dflow status working` and begin; write the plan artifact first (standard dial)
```

## Exit codes

0 success; 1 structured operational error; 2 usage error; 3 not in a dispatched context; 4 token expired/revoked.
