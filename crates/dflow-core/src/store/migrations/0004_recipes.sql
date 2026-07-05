-- Migration 0004: recipe index + privilege grants (recipes.md, security.md).
--
-- data-model.md / recipes: the markdown file is the truth; this table is an index the
-- daemon rebuilds from disk on start and on every install/change, never a second source
-- of truth. The specced columns (id, name, scope, project_id, source_path, parsed,
-- updated_at) are joined by two index-support columns, `hash` and `trust_tier`, so the
-- recipe list and the privileged-tier badge read fast without re-parsing every file.
--
-- recipe_grants (security.md / Recipe trust tiers): a privileged recipe runs on a
-- project only with an explicit per-project grant that records the file hash at grant
-- time and the exact list of elevated capabilities. A grant is valid only while the
-- current file hash matches the recorded one, so any edit to a granted recipe
-- invalidates it and forces re-confirmation.

CREATE TABLE recipes (
  id          TEXT PRIMARY KEY,            -- ulid (index row id)
  name        TEXT NOT NULL,
  scope       TEXT NOT NULL,               -- bundled | user | project
  project_id  TEXT REFERENCES projects(id),-- set for project-scoped recipes, else null
  source_path TEXT,                        -- the markdown file; null for bundled
  parsed      TEXT,                        -- cached json of the resolved recipe
  hash        TEXT,                        -- content hash of the winning file
  trust_tier  TEXT,                        -- standard | privileged
  updated_at  INTEGER
);
CREATE INDEX idx_recipes_name ON recipes(name);
CREATE UNIQUE INDEX idx_recipes_scope_name ON recipes(scope, project_id, name);

CREATE TABLE recipe_grants (
  id          TEXT PRIMARY KEY,            -- ulid
  project_id  TEXT NOT NULL REFERENCES projects(id),
  recipe_name TEXT NOT NULL,
  recipe_hash TEXT NOT NULL,               -- file hash at grant time; a change invalidates
  elevations  TEXT NOT NULL,               -- json array of exactly what was elevated
  granted_at  INTEGER NOT NULL,
  UNIQUE(project_id, recipe_name)
);
