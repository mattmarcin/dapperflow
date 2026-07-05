# Data Model Specification

SQLite (rusqlite, bundled, WAL mode).
All ids are ULIDs (sortable, sync-friendly).
Card lifecycle changes append to `card_events`; mutable tables hold current state, the event log holds card history.

Honesty note (from the 2026-07-04 review): this is an **audit-log backed local data model**, not full event sourcing.
Only card-scoped changes are evented; projects, recipes, vault entries, services, sessions, and worktrees mutate in place.
The event log gives us the timeline UI, restart reconciliation, and a head start on sync, but a real sync layer (M6+) additionally requires entity-scoped events, deterministic reducers, schema versions, tombstones, idempotency keys, and a conflict model.
That design happens when sync is built; until then no doc should claim "sync-ready" beyond stable ULIDs and this log.

## Concurrency and migrations

- All writes go through a single writer task in the daemon (mpsc channel); readers use a small connection pool.
- `busy_timeout` set defensively; every multi-statement mutation is one explicit transaction.
- Schema migrations are versioned, forward-only SQL scripts applied at daemon start; the daemon refuses to open a newer schema than it knows.

## Tables

```sql
projects (
  id TEXT PRIMARY KEY,            -- ulid
  path TEXT NOT NULL UNIQUE,      -- local repo root
  name TEXT NOT NULL,
  default_branch TEXT NOT NULL,
  mode TEXT NOT NULL DEFAULT 'pr',        -- pr | local_only
  check_cmds TEXT,                -- json array of {name, cmd} for the gate
  default_recipe TEXT,            -- recipe name
  memory_path TEXT,               -- project memory doc maintained by Concertmaster
  created_at INTEGER, updated_at INTEGER
)

cards (
  id TEXT PRIMARY KEY,
  project_id TEXT REFERENCES projects(id),  -- nullable: cross-project/none
  type TEXT NOT NULL,             -- feature | bug | chore | test | investigation
  title TEXT NOT NULL,
  lane TEXT NOT NULL DEFAULT 'inbox',     -- board column ("column" is an SQLite keyword)
  dial_recipe TEXT,               -- selected flow recipe (null -> project default)
  priority INTEGER NOT NULL DEFAULT 0,
  brief TEXT,                     -- markdown brief composed/edited during Shaping
  origin_kind TEXT NOT NULL DEFAULT 'manual',  -- manual | github_issue | audit | concertmaster | ... (generic: new sources are data)
  origin_ref TEXT,                -- github: "owner/repo#123"; audit: fingerprint slug; UNIQUE(origin_kind, origin_ref) when set, for dedupe
  origin_synced_at INTEGER,
  created_at INTEGER, updated_at INTEGER
)

card_events (
  id TEXT PRIMARY KEY,            -- ulid; doubles as the sync/stream cursor
  card_id TEXT NOT NULL REFERENCES cards(id),
  kind TEXT NOT NULL,             -- taxonomy below
  payload TEXT,                   -- json
  ts INTEGER NOT NULL
)

sessions (
  id TEXT PRIMARY KEY,
  card_id TEXT REFERENCES cards(id),  -- nullable since session-first (2026-07-04): cardless sessions are legitimate
  project_id TEXT REFERENCES projects(id),  -- direct linkage for cardless sessions (Phase 2: cwd-to-project match at create)
  cwd TEXT,                           -- spawn directory, kept for cardless resume and project re-matching
  harness TEXT NOT NULL,          -- adapter name
  model TEXT, effort TEXT,
  state TEXT NOT NULL,            -- starting|working|idle|needs_input|awaiting_feedback|blocked|done|error|interrupted
  agent_id TEXT REFERENCES agents(id),        -- the launcher used
  title TEXT,                     -- user-renamable session title (tab label); null -> generated
  worktree_id TEXT REFERENCES worktrees(id),
  scrollback_path TEXT NOT NULL,  -- persisted ring file
  resume_ref TEXT,                -- harness-native session/thread id, captured live (adapters.md), persisted immediately
  resumed_from TEXT,                  -- lineage chain: a resume creates a new row (plain column, no self-FK: SQLite ALTER + insertion-order friction, Phase 2 decision)
  first_prompt TEXT,              -- preview line for the Projects view session list
  created_at INTEGER, ended_at INTEGER
)

worktrees (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  slot INTEGER NOT NULL,
  path TEXT NOT NULL,
  lease_state TEXT NOT NULL,      -- available | leased | dirty | retired
  leased_by_card TEXT REFERENCES cards(id),
  cache_meta TEXT,                -- json: which caches verified warm
  created_at INTEGER, updated_at INTEGER
)

artifacts (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES cards(id),
  path TEXT NOT NULL,             -- html file inside the card's artifact dir
  kind TEXT NOT NULL,             -- plan | mockup | diagram | finding_review
  round INTEGER NOT NULL DEFAULT 1,
  audit TEXT,                     -- json layout-audit result
  status TEXT NOT NULL,           -- open | awaiting_feedback | approved | ended
  created_at INTEGER, updated_at INTEGER
)

annotations (
  id TEXT PRIMARY KEY,
  artifact_id TEXT NOT NULL REFERENCES artifacts(id),
  kind TEXT NOT NULL,             -- text_range | element | control | diagram_node | chat
  anchor TEXT,                    -- json anchor (selector + range offsets + quoted text)
  body TEXT,                      -- user's note or captured control value
  state TEXT NOT NULL             -- queued | sent
)

env_entries (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  key TEXT NOT NULL,              -- var name or file label
  kind TEXT NOT NULL,             -- secret | var | file
  target TEXT,                    -- for kind=file: relative path template in worktree
  ciphertext BLOB NOT NULL,       -- encrypted at rest (see environments.md)
  updated_at INTEGER,
  UNIQUE(project_id, key)
)

services (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id),
  name TEXT NOT NULL,             -- e.g. "wrangler d1", "docker compose dev"
  cmd TEXT NOT NULL,
  scope TEXT NOT NULL,            -- per_worktree | shared
  ports TEXT                      -- json port declarations for the port broker
)

recipes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  scope TEXT NOT NULL,            -- bundled | user | project
  project_id TEXT REFERENCES projects(id),
  source_path TEXT NOT NULL,      -- the markdown file; DB row is an index, file is truth
  parsed TEXT,                    -- cached json of validated frontmatter
  updated_at INTEGER
)

gate_runs (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES cards(id),
  worktree_id TEXT REFERENCES worktrees(id),
  step TEXT NOT NULL,             -- checks | review | autofix | push | pr | ci
  status TEXT NOT NULL,           -- running | passed | failed | escalated
  output_path TEXT,               -- captured logs/evidence
  started_at INTEGER, ended_at INTEGER
)

findings (
  id TEXT PRIMARY KEY,
  gate_run_id TEXT NOT NULL REFERENCES gate_runs(id),
  severity TEXT NOT NULL,         -- blocker | major | minor
  body TEXT NOT NULL,
  resolution TEXT                 -- autofixed | accepted | fixed | skipped
)

agents (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,      -- display name, e.g. "claude", "cc-alt"
  adapter TEXT NOT NULL,          -- behavior family: claude | codex | opencode | cursor | pi | custom
  command TEXT NOT NULL,          -- base executable
  extra_args TEXT,                -- json array appended at every launch
  extra_env TEXT,                 -- json object merged into the launch env
  source TEXT NOT NULL,           -- detected | custom
  detected_version TEXT,
  enabled INTEGER NOT NULL DEFAULT 1
)

needs_you_items (
  id TEXT PRIMARY KEY,
  card_id TEXT NOT NULL REFERENCES cards(id),
  kind TEXT NOT NULL,             -- plan_round | gate_finding | agent_blocked | agent_stuck | trust_dialog | pr_ready | env_drift | service_failed | audit_digest
  dedupe_key TEXT NOT NULL,       -- kind + stable subject; re-raises update, never duplicate
  score INTEGER NOT NULL,         -- computed: priority + staleness + cost_of_delay (blocked agents outrank idle ideas)
  raised_at INTEGER NOT NULL,
  notified_at INTEGER,            -- throttle: one notification per item per quiet period
  resolved_at INTEGER,
  resolved_by TEXT,               -- ui | concertmaster | auto
  UNIQUE(card_id, dedupe_key)
)
```

`needs_you_items` is a persisted projection: raised/resolved always in lockstep with `needs_you_raised`/`needs_you_resolved` events, so it can be rebuilt from the log but reads fast for ranking, dedupe, and notification throttling.

## Event taxonomy (`card_events.kind`)

Card lifecycle: `created`, `shaped`, `moved`, `dial_changed`, `closed`.
Dispatch: `dispatched`, `worktree_leased`, `env_materialized`, `brief_composed`.
Session: `session_started`, `state_changed`, `turn_ended`, `needs_input`, `blocked`, `steered`, `session_ended`.
Plan Studio: `artifact_opened`, `plan_round`, `feedback_sent`, `plan_approved`, `artifact_ended`.
Gate: `gate_started`, `gate_step`, `finding_raised`, `finding_resolved`, `gate_passed`, `gate_failed`.
Delivery: `pushed`, `pr_opened`, `ci_status`, `merged`, `worktree_returned`.
Attention: `needs_you_raised`, `needs_you_resolved`.
Concertmaster (M4): `concertmaster_steered` (payload: injected text, playbook step, session id), `round_completed` (payload: round type, scope, findings count, digest item id).
Audit (M3): `created` payloads carry audit provenance (audit card id, session id) for audit-filed cards; `audit_completed` on the audit card (payload: cards filed, notes written, budget used, digest item id).
`projects.rounds_schedule` joins `gardener_schedule` when rounds land (or both unify under one schedule table if the gardener becomes a round type, per knowledge.md).

Payloads carry evidence pointers (exit codes, log paths, PR URLs), never prose-only claims.

## Files on disk (not in DB)

- Card artifact dirs: `<app-data>/cards/<card-ulid>/artifacts/`
- Session scrollback rings: `<app-data>/scrollback/<session-ulid>.ring`
- Recipe markdown files: bundled in-app, user dir, or `<project>/.dapperflow/recipes/`
- Project memory docs: `<project>/.dapperflow/memory.md` (committed or ignored, project's choice)
- Gate evidence: `<app-data>/gate/<run-ulid>/`
