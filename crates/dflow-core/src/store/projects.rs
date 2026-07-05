//! Project registry store methods (`data-model.md` / projects).
//!
//! Projects mutate in place; they are not card-scoped, so they append no
//! `card_events` (`data-model.md` / Honesty note).

use rusqlite::params;

use dflow_proto::{CheckCmd, Project};

use super::{check_cmds_to_json, now_ms, project_from_row, Store, StoreError};

/// One row of the round-scheduler query: `(project_id, rounds_schedule, gardener_schedule)`.
pub type ProjectScheduleRow = (String, Option<String>, Option<String>);

impl Store {
    /// Insert a new project. `path` is unique; a duplicate returns a sqlite error.
    pub fn add_project(
        &self,
        path: &str,
        name: &str,
        default_branch: &str,
        mode: &str,
    ) -> Result<Project, StoreError> {
        let id = super::new_ulid().to_string();
        let ts = now_ms();
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO projects (id, path, name, default_branch, mode, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![id, path, name, default_branch, mode, ts],
            )?;
        }
        self.get_project(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("project {id} vanished after insert")))
    }

    /// Fetch a project by id.
    pub fn get_project(&self, id: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, name, default_branch, mode, check_cmds, default_recipe, \
             created_at, updated_at FROM projects WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], project_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Fetch a project by its repo path (used to reject duplicate adds cleanly).
    pub fn get_project_by_path(&self, path: &str) -> Result<Option<Project>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, name, default_branch, mode, check_cmds, default_recipe, \
             created_at, updated_at FROM projects WHERE path = ?1",
        )?;
        let mut rows = stmt.query_map(params![path], project_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// The project's `knowledge_path` override, if any (`knowledge.md` /
    /// Data-model touchpoints). `None` means use the default `docs/knowledge`.
    pub fn project_knowledge_path(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.lock();
        let path: Option<String> = conn
            .query_row("SELECT knowledge_path FROM projects WHERE id = ?1", params![id], |r| r.get(0))
            .ok()
            .flatten();
        Ok(path)
    }

    /// Set (or clear, on empty) a project's `knowledge_path` override. Returns whether
    /// a row matched.
    pub fn set_project_knowledge_path(&self, id: &str, path: &str) -> Result<bool, StoreError> {
        let value: Option<&str> = if path.trim().is_empty() { None } else { Some(path.trim()) };
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE projects SET knowledge_path = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, value, now_ms()],
        )?;
        Ok(changed > 0)
    }

    /// A project's round-schedule json (`projects.rounds_schedule`, migration 0007),
    /// or `None` when unscheduled (the default: rounds off). The `gardener` flag reads
    /// the sibling `gardener_schedule` column instead, so the gardener round type can be
    /// scheduled on its own cadence (`knowledge.md` / gardener as a round type at M4).
    pub fn project_schedule(&self, id: &str, gardener: bool) -> Result<Option<String>, StoreError> {
        let column = if gardener { "gardener_schedule" } else { "rounds_schedule" };
        let conn = self.lock();
        let value: Option<String> = conn
            .query_row(
                &format!("SELECT {column} FROM projects WHERE id = ?1"),
                params![id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        Ok(value)
    }

    /// Set (or clear, on empty) a project's round schedule json. Returns whether a row
    /// matched. `gardener` targets the `gardener_schedule` column.
    pub fn set_project_schedule(
        &self,
        id: &str,
        gardener: bool,
        schedule: Option<&str>,
    ) -> Result<bool, StoreError> {
        let column = if gardener { "gardener_schedule" } else { "rounds_schedule" };
        let value: Option<&str> = schedule.map(str::trim).filter(|s| !s.is_empty());
        let conn = self.lock();
        let changed = conn.execute(
            &format!("UPDATE projects SET {column} = ?2, updated_at = ?3 WHERE id = ?1"),
            params![id, value, now_ms()],
        )?;
        Ok(changed > 0)
    }

    /// Every project's `(id, rounds_schedule, gardener_schedule)`, for the round
    /// scheduler tick. Projects with both columns NULL (the default) are skipped by the
    /// scheduler, so an all-default fleet costs one cheap query per tick.
    pub fn list_project_schedules(&self) -> Result<Vec<ProjectScheduleRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, rounds_schedule, gardener_schedule FROM projects \
             WHERE rounds_schedule IS NOT NULL OR gardener_schedule IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// All projects, newest first.
    pub fn list_projects(&self) -> Result<Vec<Project>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, path, name, default_branch, mode, check_cmds, default_recipe, \
             created_at, updated_at FROM projects ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], project_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Update mutable project fields; absent options leave the field unchanged.
    pub fn update_project(
        &self,
        id: &str,
        mode: Option<&str>,
        check_cmds: Option<&[CheckCmd]>,
        default_recipe: Option<&str>,
    ) -> Result<Project, StoreError> {
        let existing = self
            .get_project(id)?
            .ok_or_else(|| StoreError::NotFound(format!("project {id}")))?;
        let mode = mode.unwrap_or(&existing.mode).to_string();
        let check_cmds_json = match check_cmds {
            Some(cmds) => check_cmds_to_json(cmds),
            None => check_cmds_to_json(&existing.check_cmds),
        };
        let default_recipe = default_recipe
            .map(str::to_string)
            .or_else(|| existing.default_recipe.clone());
        {
            let conn = self.lock();
            conn.execute(
                "UPDATE projects SET mode = ?2, check_cmds = ?3, default_recipe = ?4, \
                 updated_at = ?5 WHERE id = ?1",
                params![id, mode, check_cmds_json, default_recipe, now_ms()],
            )?;
        }
        self.get_project(id)?
            .ok_or_else(|| StoreError::NotFound(format!("project {id}")))
    }
}
