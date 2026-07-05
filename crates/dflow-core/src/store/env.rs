//! Env vault store methods (`data-model.md` / env_entries, `environments.md`).
//!
//! The store is deliberately crypto-agnostic: it persists and returns opaque
//! `ciphertext` bytes and never sees a plaintext value. Sealing and unsealing happen
//! one layer up in [`crate::env`] against the OS credential store, so this table
//! matches the "relay stores ciphertext only" property the E2E-sync future assumes
//! (`environments.md` / Cross-device future). Env entries are project-scoped and
//! mutate in place, so they append no `card_events` (`data-model.md` / Honesty note).

use rusqlite::{params, Row};

use super::{new_ulid, now_ms, Store, StoreError};

/// `env_entries.kind` values (`data-model.md` / env_entries).
pub mod env_kind {
    /// A plain environment variable injected into session environments.
    pub const VAR: &str = "var";
    /// Same injection, but write-only through the API (never displayed).
    pub const SECRET: &str = "secret";
    /// A materialized file written to a relative target path in the worktree.
    pub const FILE: &str = "file";

    /// Whether `kind` is one of the three known vault kinds.
    pub fn is_known(kind: &str) -> bool {
        matches!(kind, VAR | SECRET | FILE)
    }
}

/// A vault entry's metadata, without its value (`env.list`: names and kinds only,
/// never values). This is the only entry shape that ever leaves the vault toward a
/// client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvEntryMeta {
    pub key: String,
    /// `var | secret | file`.
    pub kind: String,
    /// For `file` entries: the relative target path template inside the worktree.
    pub target: Option<String>,
    /// Per-entry version, bumped on every rotate.
    pub version: i64,
    pub updated_at: Option<i64>,
}

/// A vault entry with its sealed ciphertext, for materialization only. Never
/// serialized onto the wire.
#[derive(Debug, Clone)]
pub struct EnvEntrySealed {
    pub key: String,
    pub kind: String,
    pub target: Option<String>,
    pub ciphertext: Vec<u8>,
    /// Which credential-store backend sealed this blob (e.g. `"dpapi"`).
    pub key_id: String,
}

impl Store {
    /// Upsert a vault entry's sealed ciphertext, bumping its version on replace
    /// (`env.set`). The caller has already encrypted `ciphertext` against the OS
    /// credential store; the store only persists bytes.
    pub fn set_env_entry(
        &self,
        project_id: &str,
        key: &str,
        kind: &str,
        target: Option<&str>,
        ciphertext: &[u8],
        key_id: &str,
    ) -> Result<EnvEntryMeta, StoreError> {
        let ts = now_ms();
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO env_entries \
                 (id, project_id, key, kind, target, ciphertext, key_id, version, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8) \
                 ON CONFLICT(project_id, key) DO UPDATE SET \
                   kind = excluded.kind, \
                   target = excluded.target, \
                   ciphertext = excluded.ciphertext, \
                   key_id = excluded.key_id, \
                   version = env_entries.version + 1, \
                   updated_at = excluded.updated_at",
                params![new_ulid().to_string(), project_id, key, kind, target, ciphertext, key_id, ts],
            )?;
        }
        self.get_env_entry_meta(project_id, key)?
            .ok_or_else(|| StoreError::NotFound(format!("env entry {key} vanished after insert")))
    }

    /// One entry's metadata (no value), or `None` when absent.
    pub fn get_env_entry_meta(
        &self,
        project_id: &str,
        key: &str,
    ) -> Result<Option<EnvEntryMeta>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT key, kind, target, version, updated_at FROM env_entries \
             WHERE project_id = ?1 AND key = ?2",
        )?;
        let mut rows = stmt.query_map(params![project_id, key], env_meta_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Every entry's metadata for a project, key-sorted (`env.list`: names + kinds,
    /// never values).
    pub fn list_env_entries(&self, project_id: &str) -> Result<Vec<EnvEntryMeta>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT key, kind, target, version, updated_at FROM env_entries \
             WHERE project_id = ?1 ORDER BY key ASC",
        )?;
        let rows = stmt.query_map(params![project_id], env_meta_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Every entry with its sealed ciphertext, for materialization (`env.materialize`).
    /// This is the only read path that returns the encrypted blob; it never leaves the
    /// daemon toward a client.
    pub fn env_entries_sealed(&self, project_id: &str) -> Result<Vec<EnvEntrySealed>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT key, kind, target, ciphertext, key_id FROM env_entries \
             WHERE project_id = ?1 ORDER BY key ASC",
        )?;
        let rows = stmt.query_map(params![project_id], env_sealed_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Delete a vault entry. Returns whether a row was removed.
    pub fn delete_env_entry(&self, project_id: &str, key: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "DELETE FROM env_entries WHERE project_id = ?1 AND key = ?2",
            params![project_id, key],
        )?;
        Ok(changed > 0)
    }

    /// Number of vault entries for a project (diagnostics/tests).
    pub fn count_env_entries(&self, project_id: &str) -> Result<i64, StoreError> {
        let conn = self.lock();
        let count: i64 =
            conn.query_row("SELECT count(*) FROM env_entries WHERE project_id = ?1", params![project_id], |r| r.get(0))?;
        Ok(count)
    }
}

fn env_meta_from_row(row: &Row) -> rusqlite::Result<EnvEntryMeta> {
    Ok(EnvEntryMeta {
        key: row.get(0)?,
        kind: row.get(1)?,
        target: row.get(2)?,
        version: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn env_sealed_from_row(row: &Row) -> rusqlite::Result<EnvEntrySealed> {
    Ok(EnvEntrySealed {
        key: row.get(0)?,
        kind: row.get(1)?,
        target: row.get(2)?,
        ciphertext: row.get(3)?,
        key_id: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn store_with_project() -> (Store, String) {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project("/tmp/p", "p", "main", "pr").unwrap();
        (store, project.id)
    }

    #[test]
    fn set_lists_metadata_but_not_ciphertext() {
        let (store, pid) = store_with_project();
        store.set_env_entry(&pid, "API_KEY", env_kind::SECRET, None, b"sealed-1", "dpapi").unwrap();
        store.set_env_entry(&pid, "PORT", env_kind::VAR, None, b"sealed-2", "dpapi").unwrap();
        let metas = store.list_env_entries(&pid).unwrap();
        assert_eq!(metas.len(), 2);
        // Key-sorted, and no field carries a value.
        assert_eq!(metas[0].key, "API_KEY");
        assert_eq!(metas[0].kind, env_kind::SECRET);
        assert_eq!(metas[1].key, "PORT");
    }

    #[test]
    fn set_is_upsert_and_bumps_version() {
        let (store, pid) = store_with_project();
        let first = store.set_env_entry(&pid, "TOKEN", env_kind::SECRET, None, b"v1", "dpapi").unwrap();
        assert_eq!(first.version, 1);
        let second = store.set_env_entry(&pid, "TOKEN", env_kind::SECRET, None, b"v2", "dpapi").unwrap();
        assert_eq!(second.version, 2, "a rotate must bump version, not duplicate the row");
        assert_eq!(store.count_env_entries(&pid).unwrap(), 1);
        // The sealed read returns the latest ciphertext.
        let sealed = store.env_entries_sealed(&pid).unwrap();
        assert_eq!(sealed.len(), 1);
        assert_eq!(sealed[0].ciphertext, b"v2");
    }

    #[test]
    fn delete_removes_the_entry() {
        let (store, pid) = store_with_project();
        store.set_env_entry(&pid, "GONE", env_kind::VAR, None, b"x", "dpapi").unwrap();
        assert!(store.delete_env_entry(&pid, "GONE").unwrap());
        assert!(!store.delete_env_entry(&pid, "GONE").unwrap());
        assert_eq!(store.count_env_entries(&pid).unwrap(), 0);
    }
}
