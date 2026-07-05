-- Migration 0007: M4 Concertmaster rounds + M6 LAN listener.
--
-- product.md / Concertmaster principles (Rounds): scheduled or user-triggered headless
-- Concertmaster runs, off by default, per-project schedulable. knowledge.md places the
-- Knowledge Gardener as a round type at M4 with its own per-project schedule.
--   * projects.rounds_schedule holds a json {enabled, interval_minutes, types:[...]}
--     for floor-check/general rounds; NULL means "no schedule" (the default: off).
--   * projects.gardener_schedule holds the same shape for the gardener round type.
--     Both stay unified conceptually (a gardener run is a round type) but carry two
--     columns so a project can schedule gardening on a different cadence than rounds,
--     exactly as data-model.md anticipates ("projects.rounds_schedule joins
--     gardener_schedule when rounds land").
--
-- settings is a generic key/value store for daemon-scoped, non-card settings that were
-- previously not modeled: the LAN listener toggle + port (security.md / Remote access
-- trust model: an opt-in listener, off by default), the Concertmaster launcher default
-- rounds dispatch under, and the scheduler's per-project last-run bookmarks. These are
-- process/daemon settings, not card-scoped, so they append no card_events
-- (data-model.md / Honesty note).
--
-- phone_tokens persists the phone-scoped capability tokens minted by daemon.lan.pair
-- (security.md / Remote access trust model: QR pairing mints a phone-scoped capability
-- token; per-device revocation from Settings). Persisting them (unlike the in-memory
-- Concertmaster registry) is deliberate: a paired phone must survive a daemon restart,
-- and per-device revocation must outlive one process. revoked_at NULL means live.

ALTER TABLE projects ADD COLUMN rounds_schedule TEXT;
ALTER TABLE projects ADD COLUMN gardener_schedule TEXT;

CREATE TABLE settings (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE phone_tokens (
  id           TEXT PRIMARY KEY,            -- ulid; the per-device revocation target
  token        TEXT NOT NULL UNIQUE,        -- the phone-scoped capability bearer token
  name         TEXT,                        -- optional device/label ("Matt's iPhone")
  created_at   INTEGER NOT NULL,
  last_seen_at INTEGER,                     -- stamped on each successful handshake
  revoked_at   INTEGER                      -- NULL = live; set = revoked (auth rejected)
);
CREATE INDEX idx_phone_tokens_live ON phone_tokens(revoked_at);
