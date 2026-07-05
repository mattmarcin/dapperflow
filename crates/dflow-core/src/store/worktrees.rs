//! Worktree pool store methods (`data-model.md` / worktrees).
//!
//! Worktrees mutate in place (no card events here); the pool layer
//! (`crate::worktree`) appends the `worktree_leased` / `worktree_returned` /
//! needs-you events tied to the leasing card.

use rusqlite::{params, Row};

use super::{now_ms, Store, StoreError, WorktreeRow};

impl Store {
    /// The lowest-slot `available` worktree for a project, if any.
    pub fn find_available_worktree(&self, project_id: &str) -> Result<Option<WorktreeRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "{WORKTREE_SELECT} WHERE project_id = ?1 AND lease_state = 'available' \
             ORDER BY slot ASC LIMIT 1"
        ))?;
        let mut rows = stmt.query_map(params![project_id], worktree_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// The next free slot number for a project (max existing + 1, else 0).
    pub fn next_worktree_slot(&self, project_id: &str) -> Result<i64, StoreError> {
        let conn = self.lock();
        let max: Option<i64> = conn.query_row(
            "SELECT max(slot) FROM worktrees WHERE project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )?;
        Ok(max.map(|m| m + 1).unwrap_or(0))
    }

    /// Insert a worktree row.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_worktree(
        &self,
        project_id: &str,
        slot: i64,
        path: &str,
        lease_state: &str,
        leased_by_card: Option<&str>,
        cache_meta: Option<&str>,
    ) -> Result<WorktreeRow, StoreError> {
        let id = super::new_ulid().to_string();
        let ts = now_ms();
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO worktrees \
                 (id, project_id, slot, path, lease_state, leased_by_card, cache_meta, \
                  created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![id, project_id, slot, path, lease_state, leased_by_card, cache_meta, ts],
            )?;
        }
        self.get_worktree(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("worktree {id} vanished after insert")))
    }

    /// Fetch a worktree row by id.
    pub fn get_worktree(&self, id: &str) -> Result<Option<WorktreeRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{WORKTREE_SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], worktree_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Update a worktree's lease state and holder.
    pub fn set_worktree_lease(
        &self,
        id: &str,
        lease_state: &str,
        leased_by_card: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE worktrees SET lease_state = ?2, leased_by_card = ?3, updated_at = ?4 \
             WHERE id = ?1",
            params![id, lease_state, leased_by_card, now_ms()],
        )?;
        Ok(())
    }

    /// All worktrees for a project, by slot (tests, diagnostics).
    pub fn worktrees_for_project(&self, project_id: &str) -> Result<Vec<WorktreeRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{WORKTREE_SELECT} WHERE project_id = ?1 ORDER BY slot ASC"))?;
        let rows = stmt.query_map(params![project_id], worktree_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

const WORKTREE_SELECT: &str = "SELECT id, project_id, slot, path, lease_state, leased_by_card, \
     cache_meta, created_at, updated_at FROM worktrees";

fn worktree_from_row(row: &Row) -> rusqlite::Result<WorktreeRow> {
    Ok(WorktreeRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        slot: row.get(2)?,
        path: row.get(3)?,
        lease_state: row.get(4)?,
        leased_by_card: row.get(5)?,
        cache_meta: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
