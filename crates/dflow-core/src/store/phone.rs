//! Phone-scoped capability token store (`security.md` / Remote access trust model).
//!
//! `daemon.lan.pair` mints a phone-scoped capability token and returns it inside the QR
//! pairing payload; the phone stores it and presents it on every LAN handshake. Unlike
//! the in-memory Concertmaster and per-task registries, phone tokens are **persisted**:
//! a paired phone must survive a daemon restart, and per-device revocation
//! (`daemon.lan.revoke`) must outlive one process. `revoked_at IS NULL` means live.

use rusqlite::{params, Row};

use super::{new_ulid, now_ms, Store, StoreError};

/// One persisted phone pairing (the token itself is never returned in listings).
#[derive(Debug, Clone)]
pub struct PhoneTokenRow {
    pub id: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl Store {
    /// Persist a new phone pairing for a daemon-minted `token`, returning its id (the
    /// per-device revocation target). The daemon supplies the token so all bearer-token
    /// entropy stays in one place (`dflowd::tokens`), never the store.
    pub fn add_phone_token(&self, token: &str, name: Option<&str>) -> Result<String, StoreError> {
        let id = new_ulid().to_string();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO phone_tokens (id, token, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, token, name, now_ms()],
        )?;
        Ok(id)
    }

    /// Resolve a bearer token to a live phone pairing (its label), stamping
    /// `last_seen_at`. Returns `None` for an unknown or revoked token.
    pub fn resolve_phone_token(&self, token: &str) -> Result<Option<PhoneTokenRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, created_at, last_seen_at, revoked_at FROM phone_tokens \
             WHERE token = ?1 AND revoked_at IS NULL",
        )?;
        let mut row = stmt.query_row(params![token], phone_token_from_row).ok();
        if let Some(r) = &mut row {
            let seen = now_ms();
            let _ = conn
                .execute("UPDATE phone_tokens SET last_seen_at = ?2 WHERE id = ?1", params![r.id, seen]);
            // Reflect the just-written stamp in the returned row (the read predates it).
            r.last_seen_at = Some(seen);
        }
        Ok(row)
    }

    /// List phone pairings; `include_revoked` controls whether revoked rows appear.
    pub fn list_phone_tokens(&self, include_revoked: bool) -> Result<Vec<PhoneTokenRow>, StoreError> {
        let conn = self.lock();
        let sql = if include_revoked {
            "SELECT id, name, created_at, last_seen_at, revoked_at FROM phone_tokens \
             ORDER BY created_at DESC"
        } else {
            "SELECT id, name, created_at, last_seen_at, revoked_at FROM phone_tokens \
             WHERE revoked_at IS NULL ORDER BY created_at DESC"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], phone_token_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Revoke a phone pairing by its id (per-device revocation). Returns whether a live
    /// row matched (revoking an already-revoked or unknown id is a no-op, `false`).
    pub fn revoke_phone_token(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE phone_tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now_ms()],
        )?;
        Ok(changed > 0)
    }
}

fn phone_token_from_row(row: &Row) -> rusqlite::Result<PhoneTokenRow> {
    Ok(PhoneTokenRow {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
        last_seen_at: row.get(3)?,
        revoked_at: row.get(4)?,
    })
}
