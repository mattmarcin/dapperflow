-- Migration 0001: initial schema (data-model.md).
--
-- Phase 1 populates projects, cards, card_events, sessions, worktrees, and
-- needs_you_items. The agents table (Phase 1.5 addendum) is created now, unused by
-- dispatch, so no migration is needed when the launcher lands. Tables reference only
-- tables defined above them so foreign keys resolve during creation.
--
-- Timestamp columns hold epoch milliseconds.

CREATE TABLE projects (
  id             TEXT PRIMARY KEY,
  path           TEXT NOT NULL UNIQUE,
  name           TEXT NOT NULL,
  default_branch TEXT NOT NULL,
  mode           TEXT NOT NULL DEFAULT 'pr',   -- pr | local_only
  check_cmds     TEXT,                         -- json array of {name, cmd}
  default_recipe TEXT,
  memory_path    TEXT,
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);

CREATE TABLE cards (
  id               TEXT PRIMARY KEY,
  project_id       TEXT REFERENCES projects(id),   -- nullable: cross-project/none
  type             TEXT NOT NULL,                   -- feature | bug | chore | test | investigation
  title            TEXT NOT NULL,
  lane             TEXT NOT NULL DEFAULT 'inbox',
  dial_recipe      TEXT,
  priority         INTEGER NOT NULL DEFAULT 0,
  brief            TEXT,
  origin_kind      TEXT NOT NULL DEFAULT 'manual',
  origin_ref       TEXT,
  origin_synced_at INTEGER,
  created_at       INTEGER NOT NULL,
  updated_at       INTEGER NOT NULL
);
CREATE INDEX idx_cards_project ON cards(project_id);
CREATE INDEX idx_cards_lane ON cards(lane);
-- Import dedupe: origin refs are unique per source when present.
CREATE UNIQUE INDEX idx_cards_origin ON cards(origin_kind, origin_ref)
  WHERE origin_ref IS NOT NULL;

CREATE TABLE agents (
  id               TEXT PRIMARY KEY,
  name             TEXT NOT NULL UNIQUE,
  adapter          TEXT NOT NULL,
  command          TEXT NOT NULL,
  extra_args       TEXT,                    -- json array
  extra_env        TEXT,                    -- json object
  source           TEXT NOT NULL,           -- detected | custom
  detected_version TEXT,
  enabled          INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE worktrees (
  id             TEXT PRIMARY KEY,
  project_id     TEXT NOT NULL REFERENCES projects(id),
  slot           INTEGER NOT NULL,
  path           TEXT NOT NULL,
  lease_state    TEXT NOT NULL,             -- available | leased | dirty | retired
  leased_by_card TEXT REFERENCES cards(id),
  cache_meta     TEXT,                      -- json
  created_at     INTEGER NOT NULL,
  updated_at     INTEGER NOT NULL
);
CREATE INDEX idx_worktrees_project ON worktrees(project_id, lease_state);

CREATE TABLE sessions (
  id              TEXT PRIMARY KEY,
  card_id         TEXT NOT NULL REFERENCES cards(id),
  harness         TEXT NOT NULL,
  model           TEXT,
  effort          TEXT,
  state           TEXT NOT NULL,            -- adapters.md lifecycle + interrupted
  worktree_id     TEXT REFERENCES worktrees(id),
  scrollback_path TEXT NOT NULL,
  resume_ref      TEXT,
  resumed_from    TEXT REFERENCES sessions(id),
  first_prompt    TEXT,
  title           TEXT,                     -- user-renamable tab label (Phase 1.5)
  agent_id        TEXT REFERENCES agents(id), -- Phase 1.5; unused by dispatch now
  created_at      INTEGER NOT NULL,
  ended_at        INTEGER
);
CREATE INDEX idx_sessions_card ON sessions(card_id);
CREATE INDEX idx_sessions_state ON sessions(state);

CREATE TABLE card_events (
  id      TEXT PRIMARY KEY,                 -- ulid; doubles as the stream cursor
  card_id TEXT NOT NULL REFERENCES cards(id),
  kind    TEXT NOT NULL,
  payload TEXT,                             -- json
  ts      INTEGER NOT NULL
);
CREATE INDEX idx_card_events_card ON card_events(card_id, id);

CREATE TABLE needs_you_items (
  id          TEXT PRIMARY KEY,
  card_id     TEXT NOT NULL REFERENCES cards(id),
  kind        TEXT NOT NULL,
  dedupe_key  TEXT NOT NULL,
  score       INTEGER NOT NULL,
  raised_at   INTEGER NOT NULL,
  notified_at INTEGER,
  resolved_at INTEGER,
  resolved_by TEXT,
  UNIQUE(card_id, dedupe_key)
);
