-- Migration 0007: the verification gate (gate_runs, findings) and GitHub issue-import
-- origin data (gate.md, data-model.md; phase 11 M5 core).
--
-- gate_runs / findings back the gate engine (gate.md / Pipeline): a gate run is one
-- pass of checks -> adversarial review -> autofix -> escalation -> ship for a card, in
-- an isolated gate-class worktree. data-model.md lists gate_runs(id, card_id,
-- worktree_id, step, status, output_path, started_at, ended_at) and findings(id,
-- gate_run_id, severity, body, resolution); the extra columns here (additive, like
-- services.required and cards.dismissed_at before them) carry what the engine needs to
-- run and to ship: the recipe's declared strictness, the author/reviewer harnesses (so
-- "reviewer_harness: different" can be enforced), the commit/branch under test, and the
-- PR the ship path opened. `step` tracks the current pipeline stage; `status` the run's
-- overall verdict.
--
-- findings.category routes a finding to autofix (mechanical) vs escalation (intent),
-- and findings.source distinguishes a reviewer finding from a failed check or a CI
-- failure surfaced as a finding. resolution stays null while a finding is open.
--
-- cards.origin_data holds the generic origin snapshot (github: issue body is the card
-- brief, and this json carries labels, url, state, assignees, milestone, number for the
-- card workspace's Issue tab). Modeled generically so Linear/Jira follow as data, not
-- schema changes (product.md / Card sources).

CREATE TABLE gate_runs (
  id               TEXT PRIMARY KEY,               -- ulid
  card_id          TEXT NOT NULL REFERENCES cards(id),
  worktree_id      TEXT REFERENCES worktrees(id),  -- the leased gate-class worktree
  step             TEXT NOT NULL,                  -- checks | review | autofix | escalate | push | pr | ci | done
  status           TEXT NOT NULL,                  -- running | passed | failed | escalated
  gate_strictness  TEXT,                           -- full | checks_only | none (the recipe's ask)
  author_harness   TEXT,                           -- the implementing harness (reviewer-differs check)
  reviewer_harness TEXT,                           -- the resolved adversarial reviewer harness
  head_sha         TEXT,                           -- the commit under test
  branch           TEXT,                           -- the gate/ship branch name
  pr_number        INTEGER,                        -- set when the ship path opens a PR
  pr_url           TEXT,
  output_path      TEXT,                           -- gate evidence dir <app-data>/gate/<run>/
  started_at       INTEGER,
  ended_at         INTEGER
);
CREATE INDEX idx_gate_runs_card ON gate_runs(card_id);

CREATE TABLE findings (
  id          TEXT PRIMARY KEY,                    -- ulid
  gate_run_id TEXT NOT NULL REFERENCES gate_runs(id),
  card_id     TEXT NOT NULL REFERENCES cards(id),  -- denormalized for card-scoped queries
  severity    TEXT NOT NULL,                       -- blocker | major | minor
  category    TEXT NOT NULL DEFAULT 'intent',      -- mechanical | intent (autofix routing)
  source      TEXT NOT NULL DEFAULT 'reviewer',    -- reviewer | check | ci
  body        TEXT NOT NULL,                        -- the concrete failure scenario / rule citation
  evidence    TEXT,                                -- optional path/log/citation pointer
  resolution  TEXT,                                -- autofixed | accepted | fixed | skipped (null = open)
  created_at  INTEGER,
  resolved_at INTEGER
);
CREATE INDEX idx_findings_run ON findings(gate_run_id);
CREATE INDEX idx_findings_card ON findings(card_id);

ALTER TABLE cards ADD COLUMN origin_data TEXT;
