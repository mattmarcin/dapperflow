-- Migration 0005: the per-project env vault (environments.md, data-model.md / env_entries).
--
-- Values are encrypted at rest with a key held in the OS credential store (DPAPI on
-- Windows, Keychain on macOS, Secret Service on Linux); this table stores only the
-- resulting ciphertext blob, never plaintext. The store layer is deliberately
-- crypto-agnostic: it persists and returns opaque bytes, so the "relay stores
-- ciphertext only" property the M6+ E2E-sync design assumes (environments.md /
-- Cross-device future) already holds for the local store.
--
-- The specced columns (id, project_id, key, kind, target, ciphertext, updated_at) are
-- joined by two forward-looking columns chosen now so E2E sync is an addition, not a
-- migration (environments.md: "ciphertext blob, key id, per-entry versioning"):
--   * key_id  - which credential-store key/backend sealed this blob (e.g. "dpapi").
--   * version - per-entry version, bumped on every rotate, for future conflict handling.

CREATE TABLE env_entries (
  id         TEXT PRIMARY KEY,              -- ulid
  project_id TEXT NOT NULL REFERENCES projects(id),
  key        TEXT NOT NULL,                 -- var name or file label
  kind       TEXT NOT NULL,                 -- secret | var | file
  target     TEXT,                          -- for kind=file: relative path template in the worktree
  ciphertext BLOB NOT NULL,                 -- encrypted at rest (see environments.md)
  key_id     TEXT NOT NULL DEFAULT 'dpapi', -- credential-store backend id that sealed the blob
  version    INTEGER NOT NULL DEFAULT 1,    -- per-entry version, bumped on rotate
  updated_at INTEGER,
  UNIQUE(project_id, key)
);
CREATE INDEX idx_env_entries_project ON env_entries(project_id);
