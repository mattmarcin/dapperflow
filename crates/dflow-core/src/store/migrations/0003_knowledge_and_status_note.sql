-- Migration 0003: M2 knowledgebase index + tier-1 status note + knowledge_path.
--
-- knowledge.md / Data-model touchpoints:
--   * projects.knowledge_path replaces memory_path (memory_path stays readable until
--     M4; this migration only adds the new column, it does not drop the old one).
--   * knowledge_notes is a new index table (file is truth, DB row is a fast index for
--     `dflow know find` and the in-app viewer); rebuilt from disk on daemon start and
--     on every write, never a second source of truth.
--
-- agent-cli.md / product.md session strip: sessions.status_note holds the agent's last
-- tier-1 status note (`dflow status`/`dflow card note`) that the board subtitles read.

ALTER TABLE projects ADD COLUMN knowledge_path TEXT;

ALTER TABLE sessions ADD COLUMN status_note TEXT;

CREATE TABLE knowledge_notes (
  id          TEXT PRIMARY KEY,            -- ulid (index row id, not the note identity)
  project_id  TEXT NOT NULL REFERENCES projects(id),
  path        TEXT NOT NULL,               -- relative to the knowledge root, e.g. "decisions/x.md"
  type        TEXT NOT NULL DEFAULT 'note',
  title       TEXT,
  description TEXT,
  tags        TEXT,                         -- json array of extracted tags, cached for find
  source_card TEXT,                         -- provenance if agent-written
  updated_at  INTEGER,
  UNIQUE(project_id, path)
);
CREATE INDEX idx_knowledge_notes_project ON knowledge_notes(project_id);
CREATE INDEX idx_knowledge_notes_type ON knowledge_notes(project_id, type);
