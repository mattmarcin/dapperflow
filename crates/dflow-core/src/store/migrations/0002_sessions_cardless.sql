-- Migration 0002: sessions.card_id nullable + cardless-session linkage.
--
-- data-model.md marks sessions.card_id nullable "since session-first (2026-07-04):
-- cardless sessions are legitimate". SQLite cannot drop a NOT NULL constraint in
-- place, so the table is rebuilt (the documented 12-step rebuild).
--
-- Phase 2 divergence from data-model.md's sessions table (recorded in
-- the design notes): two columns are added so a bare session.create
-- session survives a daemon restart with its Projects-tree identity intact and can be
-- resumed:
--   * project_id  - cwd -> project match captured at create (cardless sessions have
--                    no card to derive a project from);
--   * cwd         - the working directory, needed to relaunch on the same path.
-- The resumed_from self-FK is dropped (lineage stays app-managed) so the rebuild
-- needs no foreign_key toggling inside the migration transaction; nothing else
-- references sessions, and every other FK is preserved.

CREATE TABLE sessions_new (
  id              TEXT PRIMARY KEY,
  card_id         TEXT REFERENCES cards(id),      -- nullable since session-first
  project_id      TEXT REFERENCES projects(id),   -- cwd->project match (Phase 2)
  cwd             TEXT,                            -- working dir for resume (Phase 2)
  harness         TEXT NOT NULL,
  model           TEXT,
  effort          TEXT,
  state           TEXT NOT NULL,
  worktree_id     TEXT REFERENCES worktrees(id),
  scrollback_path TEXT NOT NULL,
  resume_ref      TEXT,
  resumed_from    TEXT,                            -- lineage chain (app-managed, no FK)
  first_prompt    TEXT,
  title           TEXT,
  agent_id        TEXT REFERENCES agents(id),
  created_at      INTEGER NOT NULL,
  ended_at        INTEGER
);

INSERT INTO sessions_new
  (id, card_id, project_id, cwd, harness, model, effort, state, worktree_id,
   scrollback_path, resume_ref, resumed_from, first_prompt, title, agent_id,
   created_at, ended_at)
SELECT
  id, card_id, NULL, NULL, harness, model, effort, state, worktree_id,
  scrollback_path, resume_ref, resumed_from, first_prompt, title, agent_id,
  created_at, ended_at
FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE INDEX idx_sessions_card ON sessions(card_id);
CREATE INDEX idx_sessions_state ON sessions(state);
CREATE INDEX idx_sessions_project ON sessions(project_id);
