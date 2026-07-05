//! Generic daemon settings key/value store (`data-model.md` / added at migration 0007).
//!
//! Daemon-scoped, non-card settings: the opt-in LAN listener toggle and port
//! (`security.md` / Remote access trust model), the Concertmaster launcher default
//! rounds dispatch under, and the round scheduler's per-project last-run bookmarks.
//! These mutate in place and append no `card_events` (`data-model.md` / Honesty note).

use rusqlite::params;

use super::{now_ms, Store, StoreError};

/// Well-known setting keys, so callers never hand-roll a string.
pub mod setting_key {
    /// `"1"` when the opt-in LAN listener is enabled (`security.md`).
    pub const LAN_ENABLED: &str = "lan.enabled";
    /// The LAN listener port (decimal string); `daemon.lan.enable` persists it.
    pub const LAN_PORT: &str = "lan.port";
    /// The launcher (agent name/id) rounds dispatch under when a round names none
    /// (`product.md` / Concertmaster; the "concertmaster launcher setting").
    pub const CONCERTMASTER_LAUNCHER: &str = "concertmaster.launcher";
}

impl Store {
    /// Read a setting value, or `None` when unset.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.lock();
        let value: Option<String> = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .ok();
        Ok(value)
    }

    /// Write (upsert) a setting value.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now_ms()],
        )?;
        Ok(())
    }

    /// Delete a setting; returns whether a row matched.
    pub fn delete_setting(&self, key: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(changed > 0)
    }

    /// A boolean setting: `true` only when the stored value is exactly `"1"`.
    pub fn get_bool_setting(&self, key: &str) -> Result<bool, StoreError> {
        Ok(self.get_setting(key)?.as_deref() == Some("1"))
    }
}
