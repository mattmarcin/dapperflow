# Knowledgebase Specification

**Status: APPROVED 2026-07-04** (user decisions recorded at the end).
Grounding research: `the design notes` (OKF verdict, prior-art survey, sources).

## Concept: three memory layers

DapperFlow gives agents three memory layers with distinct lifetimes:

1. **Working memory: cards and `card_events`.** Task-scoped, event-sourced, evidence-bearing; it is what an agent needs while a card is in flight, and it goes cold when the card is Done.
2. **Durable project memory: the knowledgebase.** A per-project directory of markdown notes that outlives any card; conventions, decisions, gotchas, runbooks, architecture facts; co-authored by agents, the Concertmaster, and the user.
3. **Cross-project memory: the Concertmaster.** What the orchestrator knows across the fleet; deferred, see Out of scope.

The knowledgebase is layer 2.
Its contract in one sentence: a folder of plain markdown files that Obsidian opens today with zero configuration, that agents read and write through token-efficient `dflow` verbs, and that git versions and syncs like any other part of the repo.

## Where it lives

- Default location: `<project>/docs/knowledge/` (user decision 2026-07-04): visible, first-class project docs, browsable on GitHub, and inside a repo-root Obsidian vault by default (Obsidian ignores dot-directories, which ruled out `.dapperflow/` as the default).
- Per-project override: `projects.knowledge_path` (see Data-model touchpoints) for projects that want it elsewhere, including tucked into `.dapperflow/knowledge/`.
- Committed by default; a project that wants it local-only gitignores it, same stance as recipes (project's choice, DapperFlow does not care).
- The directory is created lazily: first note wins, no empty scaffolding on project registration.

### Relationship to `.dapperflow/memory.md`

`knowledge/index.md` **absorbs** the role of `memory.md`.
The memory digest that dispatch injects into briefs (`dflow card` shows it as `memory digest:`) is sourced from the digest section of `index.md` from M2 on.
Back-compat: if `memory.md` exists and `knowledge/` does not, the daemon treats `memory.md` as the digest source; the Concertmaster offers a one-time migration (move content into `knowledge/index.md`, leave a pointer).
`projects.memory_path` is deprecated in favor of `knowledge_path` once this spec lands.

## Directory layout

```
docs/knowledge/
├── index.md              # digest + catalog; the token-efficient entry point
├── decisions/
│   └── soft-delete-accounts.md
├── conventions/
│   └── error-handling.md
├── gotchas/
│   └── conpty-resize-storm.md
└── runbooks/
    └── release.md
```

- Subdirectories are conventional, not enforced: `decisions/`, `conventions/`, `gotchas/`, `runbooks/`, `reference/` are the suggested set, but any layout is valid.
- One note per file, kebab-case filenames, `.md` only.
- A note's identity is its path relative to the knowledge root, minus `.md` (`decisions/soft-delete-accounts`); renames are edits to identity and tooling treats them as such (link rewriting is the editor's job, wikilink resolution tolerates staleness).

### `index.md`

The single file an agent can afford to read every time.
Structure:

```markdown
# Project knowledge - acme-web

## Digest
<= 30 lines of the highest-value facts, maintained by the Concertmaster.
This section is what dispatch injects into briefs.

## Catalog
- decisions (4): [[decisions/soft-delete-accounts]] - accounts soft-delete, 90-day purge; ...
- gotchas (2): [[gotchas/conpty-resize-storm]] - debounce resize on Windows; ...
```

- The Digest section is hand-curated (by the Concertmaster or the user), hard-capped at 30 lines; the daemon truncates beyond the cap when injecting into briefs.
- The Catalog section is regenerated deterministically from the note files (type, path, description); regeneration is idempotent, so merge conflicts in it are resolved by regenerating.

## File format

Markdown with YAML frontmatter.

```markdown
---
type: decision
title: Soft-delete for accounts
description: Accounts are soft-deleted with a 90-day purge job; hard delete only via support tooling.
tags: [accounts, data-retention]
card: 01JX3F...        # optional provenance: the card that produced this note
updated: 2026-07-04
---

## Decision
Soft-delete with `deleted_at`, purge job after 90 days.

## Why
Support recovery cases outweighed storage cost; see [[conventions/error-handling]] for the surfaced error copy.
```

Field rules (OKF-inspired, deliberately permissive):

- `type` is the only field tooling relies on; suggested vocabulary: `decision | convention | gotcha | runbook | reference | note`; unknown types are valid and listed under their own heading in the catalog.
- `title`, `description`, `tags`, `card`, `updated` are recommended, never required.
- Producers (agents, users, plugins) may add any keys; every DapperFlow reader preserves unknown keys and **never rejects a note** for missing fields, unknown types, unknown keys, or broken links.
- A note with no frontmatter at all is still a note (type defaults to `note`); users must be able to drop a plain markdown file in and have it count.

Links: `[[wikilinks]]` and relative markdown links are both first-class; DapperFlow tooling resolves both, and never rewrites one style into the other in place.
Obsidian-specific block references are tolerated but no DapperFlow feature may require them.

### OKF posture

The format above is OKF-adjacent by construction (markdown + frontmatter, `type`, path-as-identity, permissive readers) but **not** conformant, because wikilinks are first-class and `type` is defaulted rather than required.
If OKF gains multi-vendor adoption, `dflow know export --okf <dir>` is a mechanical transform: rewrite wikilinks to bundle-relative markdown links, stamp `type` on frontmatter-less notes, emit per-directory `index.md` listings.
No DapperFlow surface depends on OKF; the exporter is an option we keep cheap, not a commitment.
Reasoning and evidence in `the design notes`.

## Obsidian compatibility contract

What DapperFlow guarantees so that "point Obsidian at it" stays true:

1. Every file is plain markdown; no sidecar databases, no binary formats, no required plugins.
2. Frontmatter is plain YAML that Obsidian renders as properties; `tags` uses the list form Obsidian understands natively.
3. Wikilinks written by Obsidian are resolved by all DapperFlow tooling; DapperFlow-written notes use wikilinks by default so graph view and backlinks work.
4. DapperFlow never fights the user's editor: no file watchers that rewrite user edits, no canonical-formatting pass over note bodies; the only file the daemon regenerates is the Catalog section of `index.md`.
5. `.obsidian/` (vault config) is left alone and recommended for the project's gitignore.
6. External edits are picked up on read; the daemon holds no lock on the directory.

## How agents use it: `dflow know`

New verb family in the agent CLI (extends the table in `agent-cli.md`; same AXI rules: token-minimal, aggregates pre-computed, definitive empty states, `next:` lines).

| Verb | Purpose | Notes |
|---|---|---|
| `dflow know` | Digest + catalog counts | Content first; this is the whole index at a glance |
| `dflow know find <query>` | Search titles, tags, descriptions, then bodies | Compact table: id, type, description; `--type` filter |
| `dflow know get <id>` | Print one note | Truncated with size hint; `--full` escape hatch |
| `dflow know add --type <t> --title <t> [--stdin\|--file <f>]` | Create a note | Id derived from type + title; provenance `card:` stamped automatically from `DFLOW_CARD`; idempotent per id (re-add updates) |

Example outputs:

```
$ dflow know
digest (acme-web, 12 lines): tailwind, next-themes NOT installed; accounts soft-delete 90d; ...
catalog: 4 decisions, 3 conventions, 2 gotchas, 1 runbook
next: `dflow know find <query>` before re-deriving anything; `dflow know add` when you learn something durable
```

```
$ dflow know find retention
2 notes:
  decisions/soft-delete-accounts  decision  accounts soft-delete, 90-day purge
  runbooks/purge-job              runbook   manual purge-job runbook
next: `dflow know get decisions/soft-delete-accounts`
```

```
$ dflow know find webhooks
no notes match
next: if you derive the answer, record it: `dflow know add --type gotcha --title "..." --stdin`
```

Write path mechanics:

- **Write policy (user decision 2026-07-04): autonomous and evidenced.** Worker agents write notes directly during any card; there is no approval queue. Git makes every write reviewable and revertable, the `knowledge_updated` event makes it visible on the timeline, and curation (below) catches drift. Signal quality is the gardener's and Concertmaster's job, not a human review chore.
- **Commit policy (user decision 2026-07-04): the daemon never commits.** Writes land as ordinary dirty files in the project root; they ride along in the user's next commit or a card's ship path. No daemon-authored commits appear in project history.
- `know add` goes through the daemon (same WS + scoped token as every verb), which writes the file, regenerates the Catalog section of `index.md`, and appends a `knowledge_updated` event to the current card (path, type, title in the payload) so the timeline shows what the agent recorded and when.
- Agents edit only via `know add` (create or replace by id); free-form file editing in the worktree is not the write path, because the knowledgebase lives in the project repo, not in the leased worktree's diff (see next section).
- The Digest section is never writable by worker agents; curating it is gardener, Concertmaster, and user territory.

## The Knowledge Gardener (curation sweeps)

Agents get distracted and forget bookkeeping mid-task; relying on in-task discipline alone guarantees a stale knowledgebase (user insight 2026-07-04, pattern borrowed from Hermes-style periodic agent runs).
So curation is a first-class, dispatchable activity:

- A **gardener run** is an ordinary agent session (user-chosen launcher) with a built-in brief: audit recent activity against the knowledgebase and fix drift.
  Inputs: `card_events` since the last run (completed cards, gate findings, blocked notes, `knowledge_updated` events), the current catalog, and the digest.
  Actions, all through the normal verbs: merge duplicate notes, record obviously-missing learnings from completed cards, flag stale notes (a `stale: true` frontmatter key plus a Needs You item for deletion candidates - the gardener never deletes), and propose Digest updates.
- **Triggers**: user-initiated from the project's knowledge tab ("Garden now"), and optionally scheduled per project (`projects.gardener_schedule`, e.g. weekly); scheduled runs are off by default and surface their outcome as a single Needs You digest item, never silent.
- Gardener runs are sessions like any other: visible terminal, timeline events, one click away; nothing about them is a hidden batch job.
- The Concertmaster (M4) subsumes scheduling judgment (it can decide a garden is due from fleet activity), but the gardener itself works from M2 with no Concertmaster.
- At M4, gardener runs become a round type under the Concertmaster rounds mechanism (product.md, Concertmaster principles); M2 behavior is unchanged.

### Worktree note

The knowledgebase lives in the project repo, and leased worktrees carry a checkout of it like any other tracked path.
Reads during a session come from the worktree (consistent with the code the agent sees).
Writes via `know add` land in the **project root checkout** through the daemon, not in the worktree diff: knowledge updates are not review-gated code changes, and they must survive worktree teardown regardless of whether the card's branch ever merges.
Consequence, stated honestly: a knowledge write is visible to the writing agent's own worktree only after its next pull, and the note commit is the user's to make (or the Concertmaster's, per the write-policy open question below).

## Cards and the knowledgebase (promotion)

- During a card: the brief carries the digest (`dflow card`); the agent consults `know find` before re-deriving project facts.
- At card completion: the recipe's ship stage may include a **distill** hint in its guidance text, prompting the agent to record durable learnings via `know add` (with `card:` provenance stamped automatically).
- The Concertmaster periodically reviews recent `knowledge_updated` events, merges duplicates, promotes recurring gotchas into the Digest, and prunes stale notes; every such change is an ordinary git-visible file edit.
- Cards remain the working-memory layer: nothing card-scoped (feedback rounds, gate findings, session chatter) belongs in the knowledgebase unless distilled.
- The onboarding audit (product.md, card sources) is a knowledge seeder: typed, note-budget-capped, evidence-cited notes, and it can never touch the Digest. Dev-loop runbooks it writes may carry a `check_cmds` frontmatter convention that the daemon reads to prefill the project's check-command proposal (confirmed by the user in settings, never auto-applied). The gardener treats audit-authored notes as ordinary notes for staleness.

## Data-model touchpoints

- `projects.knowledge_path TEXT` replaces `memory_path` (migration keeps `memory_path` readable until M4).
- New `card_events.kind`: `knowledge_updated` (payload: note path, type, title, verb) under the Session group.
- New index table, following the recipes pattern (file is truth, DB row is an index):

```sql
knowledge_notes (
  id TEXT PRIMARY KEY,            -- ulid
  project_id TEXT NOT NULL REFERENCES projects(id),
  path TEXT NOT NULL,             -- relative to knowledge root, e.g. "decisions/soft-delete-accounts.md"
  type TEXT NOT NULL DEFAULT 'note',
  title TEXT, description TEXT, tags TEXT,   -- extracted frontmatter, cached for find
  source_card TEXT,               -- provenance if agent-written
  updated_at INTEGER,
  UNIQUE(project_id, path)
)
```

- The table is rebuilt from the directory on daemon start and on write; it exists so `know find` and the in-app viewer are fast, never as a second source of truth.

## Sync and conflict posture

- Git is the sync layer and the conflict layer; the daemon adds none of its own.
- Within one machine, all daemon writes serialize through the single writer task, so concurrent `know add` from parallel sessions cannot corrupt files.
- Across machines or branches, notes are per-file and rarely co-edited, so git merges cleanly in practice; the one hot file, `index.md`, is designed for it (Digest is small and human-owned, Catalog regenerates idempotently, so the resolution for any Catalog conflict is "regenerate").
- The event log records knowledge writes for timeline and audit purposes only; the files are the truth, consistent with the data-model honesty note (this is not full event sourcing, and knowledge does not pretend otherwise).

## Milestones

- **M2 (Ensemble)**: directory conventions, `knowledge_notes` index, digest injection into briefs, read verbs (`know`, `know find`, `know get`), **plus** `know add` + `knowledge_updated` events and the user-triggered gardener run (writes moved up from M4 because the write policy is autonomous: agents maintaining knowledge is the point, not an afterthought). Obsidian is the only editor.
- **M4 (Concertmaster)**: Concertmaster curation of the Digest via MCP; scheduled gardener judgment; migration from `memory.md`; in-app **read-only** knowledge tab in the Projects view (rendered markdown, wikilink navigation, backlinks list).
- **M6+ (post-Encore)**: in-app editing, graph view if demand exists, `--okf` exporter if OKF earns adoption.

## Out of scope (explicit)

- Embeddings, vector search, or any RAG index; `know find` is substring/tag search over a few hundred notes, and that is enough at this scale.
- Typed link semantics or graph queries; links stay untyped edges, per the convergence finding in the research doc.
- Cross-project or global knowledgebase; the Concertmaster's fleet-level memory is a separate design for M4+, and per-project notes never auto-replicate across projects.
- Two-way sync with external wikis, Notion, or catalog products.
- OKF conformance as a storage requirement (export-only, and only if warranted later).
- Enforced taxonomies, required fields beyond none, or validation gates on note content.

## Decisions record (2026-07-04, user)

1. **Write policy**: autonomous and evidenced; no approval queue. Plus the Knowledge Gardener (above) for drift, user-triggered first, schedulable later.
2. **Who commits**: the user; the daemon never commits knowledge writes.
3. **Default location**: `docs/knowledge/` (visible, repo-root-vault friendly); `.dapperflow/` stays machinery-only.
4. **memory.md migration**: approved; `knowledge/index.md` absorbs it per the back-compat path above.
5. **Type vocabulary**: suggested set, never enforced (orchestrator default; matches the permissive-reader posture).
6. **Milestone placement**: reads AND writes at M2 (write policy made writes the point); curation intelligence at M4 (orchestrator default given the write decision).
