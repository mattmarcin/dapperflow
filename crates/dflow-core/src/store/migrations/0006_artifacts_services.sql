-- Migration 0006: Plan Studio artifacts + annotations, local services, and durable
-- audit dismissals (plan-studio.md, environments.md, data-model.md; phase 10 M3 core).
--
-- artifacts / annotations back the Plan Studio review loop: an agent registers a plan
-- HTML artifact, the daemon serves it over signed loopback URLs, the human annotates in
-- the review chrome, and the agent polls for the queued feedback batch. The specced
-- columns (id, card_id, path, kind, round, audit, status) are joined by three the UI
-- codes against (phase5-m3-ui.md / Interpretations): title, doc_id (the serving identity
-- the artifact HTTP endpoint signs), and revised_doc_id (the revise-in-place nonce the
-- iframe reloads on).
--
-- services backs the per-worktree port broker (environments.md / Local services): the
-- daemon starts declared per_worktree services at dispatch, allocating real free ports
-- injected as DFLOW_PORT_<NAME>. The `required` column (additive beyond data-model.md's
-- listed columns) lets an optional service fail without parking the card.
--
-- cards.dismissed_at makes an audit-origin card's dismissal durable, so a re-audit's
-- fingerprint dedupe suppresses refiling a finding the human already dismissed
-- (aligns with the cards UNIQUE(origin_kind, origin_ref) semantics).

CREATE TABLE artifacts (
  id             TEXT PRIMARY KEY,           -- ulid
  card_id        TEXT NOT NULL REFERENCES cards(id),
  path           TEXT NOT NULL,              -- the agent's source path (idempotency key with card)
  kind           TEXT NOT NULL,              -- plan | mockup | diagram | finding_review
  title          TEXT,                       -- human title for the artifact tab (UI ArtifactMeta)
  doc_id         TEXT NOT NULL,              -- stable serving identity the HTTP endpoint signs
  revised_doc_id TEXT,                       -- per-revision nonce; the iframe reloads when it changes
  round          INTEGER NOT NULL DEFAULT 1, -- review round; increments on each revision open
  audit          TEXT,                       -- json layout-audit result (the latest layout_warnings)
  status         TEXT NOT NULL,              -- open | awaiting_feedback | approved | ended
  created_at     INTEGER,
  updated_at     INTEGER,
  UNIQUE(card_id, path)
);
CREATE INDEX idx_artifacts_card ON artifacts(card_id);
CREATE UNIQUE INDEX idx_artifacts_doc ON artifacts(doc_id);

CREATE TABLE annotations (
  id          TEXT PRIMARY KEY,              -- ulid
  artifact_id TEXT NOT NULL REFERENCES artifacts(id),
  round       INTEGER NOT NULL DEFAULT 1,    -- the review round this item was submitted for
  kind        TEXT NOT NULL,                 -- text_range | element | control | diagram_node | action | chat
  anchor      TEXT,                          -- json anchor (selector + range offsets + quoted text)
  body        TEXT,                          -- user's note or captured control value (string form)
  payload     TEXT NOT NULL,                 -- the full FeedbackItem json (replayed to the agent poll)
  state       TEXT NOT NULL DEFAULT 'queued' -- queued (undelivered) | sent (delivered to the agent)
);
CREATE INDEX idx_annotations_artifact ON annotations(artifact_id, round);

CREATE TABLE services (
  id         TEXT PRIMARY KEY,               -- ulid
  project_id TEXT NOT NULL REFERENCES projects(id),
  name       TEXT NOT NULL,                  -- e.g. "wrangler d1", "docker compose dev"
  cmd        TEXT NOT NULL,                  -- shell command; {DFLOW_PORT_<NAME>} substituted at start
  scope      TEXT NOT NULL DEFAULT 'per_worktree', -- per_worktree | shared (shared is M4+)
  ports      TEXT,                           -- json array of port declarations for the port broker
  required   INTEGER NOT NULL DEFAULT 1,     -- a failed required service parks the card in Needs You
  UNIQUE(project_id, name)
);
CREATE INDEX idx_services_project ON services(project_id);

ALTER TABLE cards ADD COLUMN dismissed_at INTEGER;
