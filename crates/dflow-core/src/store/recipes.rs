//! The `recipes` index and `recipe_grants` store methods (`data-model.md` / recipes,
//! `security.md` / Recipe trust tiers).
//!
//! The markdown files are the truth; the `recipes` table is a fast index the daemon
//! rebuilds from disk on start and on install/change (never a second source of truth).
//! `recipe_grants` records a privileged recipe's per-project consent, keyed to the file
//! hash so an edit forces re-confirmation. Neither table is card-scoped, so they append
//! no `card_events` (`data-model.md` / Honesty note).

use rusqlite::{params, Row};

use super::{new_ulid, now_ms, Store, StoreError};

/// One row for the recipe index (a projection of a resolved recipe file).
#[derive(Debug, Clone)]
pub struct RecipeIndexRow {
    pub name: String,
    /// `bundled | user | project`.
    pub scope: String,
    pub project_id: Option<String>,
    pub source_path: Option<String>,
    /// Cached JSON of the resolved recipe.
    pub parsed_json: Option<String>,
    pub hash: Option<String>,
    /// `standard | privileged`.
    pub trust_tier: Option<String>,
}

/// A recorded per-project privilege grant for a recipe (`security.md`).
#[derive(Debug, Clone)]
pub struct RecipeGrant {
    pub project_id: String,
    pub recipe_name: String,
    /// The recipe file's content hash at grant time; a change invalidates the grant.
    pub recipe_hash: String,
    /// JSON array of the elevated capabilities the human approved.
    pub elevations: String,
    pub granted_at: i64,
}

impl Store {
    /// Replace the whole recipe index with a fresh scan (`data-model.md`: rebuilt from
    /// disk on start and on change). One transaction so a reader never sees a half-index.
    pub fn replace_recipe_index(&self, rows: &[RecipeIndexRow]) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM recipes", [])?;
        let ts = now_ms();
        for row in rows {
            tx.execute(
                "INSERT INTO recipes \
                 (id, name, scope, project_id, source_path, parsed, hash, trust_tier, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    new_ulid().to_string(),
                    row.name,
                    row.scope,
                    row.project_id,
                    row.source_path,
                    row.parsed_json,
                    row.hash,
                    row.trust_tier,
                    ts,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The whole recipe index, name-sorted (diagnostics and the recipe list fallback).
    pub fn list_recipe_index(&self) -> Result<Vec<RecipeIndexRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT name, scope, project_id, source_path, parsed, hash, trust_tier \
             FROM recipes ORDER BY name ASC, scope ASC",
        )?;
        let rows = stmt.query_map([], recipe_index_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Record (or refresh) a per-project privilege grant for a recipe. Upserts on
    /// `(project_id, recipe_name)`, so re-granting after a hash change updates in place.
    pub fn grant_recipe(
        &self,
        project_id: &str,
        recipe_name: &str,
        recipe_hash: &str,
        elevations_json: &str,
    ) -> Result<RecipeGrant, StoreError> {
        let ts = now_ms();
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO recipe_grants (id, project_id, recipe_name, recipe_hash, elevations, granted_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(project_id, recipe_name) DO UPDATE SET \
                   recipe_hash = excluded.recipe_hash, \
                   elevations = excluded.elevations, \
                   granted_at = excluded.granted_at",
                params![new_ulid().to_string(), project_id, recipe_name, recipe_hash, elevations_json, ts],
            )?;
        }
        self.recipe_grant(project_id, recipe_name)?
            .ok_or_else(|| StoreError::NotFound("recipe grant vanished after insert".into()))
    }

    /// The recorded grant for a recipe on a project, if any.
    pub fn recipe_grant(
        &self,
        project_id: &str,
        recipe_name: &str,
    ) -> Result<Option<RecipeGrant>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT project_id, recipe_name, recipe_hash, elevations, granted_at \
             FROM recipe_grants WHERE project_id = ?1 AND recipe_name = ?2",
        )?;
        let mut rows = stmt.query_map(params![project_id, recipe_name], recipe_grant_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Whether a valid grant exists: a recorded grant whose hash matches the current
    /// file hash (`security.md`: re-confirmed when the file's hash changes).
    pub fn has_valid_recipe_grant(
        &self,
        project_id: &str,
        recipe_name: &str,
        current_hash: &str,
    ) -> Result<bool, StoreError> {
        Ok(self
            .recipe_grant(project_id, recipe_name)?
            .is_some_and(|g| g.recipe_hash == current_hash))
    }

    /// Revoke a recipe grant. Returns whether a row was removed.
    pub fn revoke_recipe_grant(&self, project_id: &str, recipe_name: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "DELETE FROM recipe_grants WHERE project_id = ?1 AND recipe_name = ?2",
            params![project_id, recipe_name],
        )?;
        Ok(changed > 0)
    }
}

fn recipe_index_from_row(row: &Row) -> rusqlite::Result<RecipeIndexRow> {
    Ok(RecipeIndexRow {
        name: row.get(0)?,
        scope: row.get(1)?,
        project_id: row.get(2)?,
        source_path: row.get(3)?,
        parsed_json: row.get(4)?,
        hash: row.get(5)?,
        trust_tier: row.get(6)?,
    })
}

fn recipe_grant_from_row(row: &Row) -> rusqlite::Result<RecipeGrant> {
    Ok(RecipeGrant {
        project_id: row.get(0)?,
        recipe_name: row.get(1)?,
        recipe_hash: row.get(2)?,
        elevations: row.get(3)?,
        granted_at: row.get(4)?,
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
    fn index_replace_is_atomic_and_readable() {
        let (store, pid) = store_with_project();
        let rows = vec![
            RecipeIndexRow {
                name: "standard".into(),
                scope: "bundled".into(),
                project_id: None,
                source_path: None,
                parsed_json: Some("{}".into()),
                hash: Some("abc".into()),
                trust_tier: Some("standard".into()),
            },
            RecipeIndexRow {
                name: "myflow".into(),
                scope: "project".into(),
                project_id: Some(pid.clone()),
                source_path: Some("/tmp/p/.dapperflow/recipes/myflow.md".into()),
                parsed_json: Some("{}".into()),
                hash: Some("def".into()),
                trust_tier: Some("privileged".into()),
            },
        ];
        store.replace_recipe_index(&rows).unwrap();
        let listed = store.list_recipe_index().unwrap();
        assert_eq!(listed.len(), 2);
        // A second replace fully supersedes the first (no accumulation).
        store.replace_recipe_index(&rows[..1]).unwrap();
        assert_eq!(store.list_recipe_index().unwrap().len(), 1);
    }

    #[test]
    fn grant_records_hash_and_invalidates_on_change() {
        let (store, pid) = store_with_project();
        assert!(!store.has_valid_recipe_grant(&pid, "privy", "hash1").unwrap());
        let grant = store.grant_recipe(&pid, "privy", "hash1", "[{\"kind\":\"worktree_in_place\"}]").unwrap();
        assert_eq!(grant.recipe_hash, "hash1");
        // Valid while the hash matches.
        assert!(store.has_valid_recipe_grant(&pid, "privy", "hash1").unwrap());
        // A file edit (new hash) invalidates the grant.
        assert!(!store.has_valid_recipe_grant(&pid, "privy", "hash2").unwrap());
        // Re-granting under the new hash restores validity (upsert, not a duplicate).
        store.grant_recipe(&pid, "privy", "hash2", "[]").unwrap();
        assert!(store.has_valid_recipe_grant(&pid, "privy", "hash2").unwrap());
        // Revoke removes it.
        assert!(store.revoke_recipe_grant(&pid, "privy").unwrap());
        assert!(!store.has_valid_recipe_grant(&pid, "privy", "hash2").unwrap());
    }
}
