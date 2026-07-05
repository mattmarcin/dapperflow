//! Local service store methods (`data-model.md` / services, `environments.md` / Local
//! services and the port broker).
//!
//! Services mutate in place (project-scoped, no card events). The pool + port broker
//! (`crate::service`) reads these rows at dispatch and starts the per-worktree ones.

use rusqlite::{params, Row};

use dflow_proto::ServiceInfo;

use super::{new_ulid, Store, StoreError};

/// `services.scope` values (`data-model.md` / services).
pub mod service_scope {
    /// One instance per leased worktree (`wrangler dev`, `npm run dev`).
    pub const PER_WORKTREE: &str = "per_worktree";
    /// A daemon-managed singleton, refcounted (M4+).
    pub const SHARED: &str = "shared";

    /// Whether `scope` is a known value.
    pub fn is_known(scope: &str) -> bool {
        matches!(scope, PER_WORKTREE | SHARED)
    }
}

/// A row of the `services` table.
#[derive(Debug, Clone)]
pub struct ServiceRow {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    pub scope: String,
    /// Named port declarations for the port broker (parsed from the `ports` json).
    pub ports: Vec<String>,
    pub required: bool,
}

impl ServiceRow {
    /// The wire `ServiceInfo`.
    pub fn to_info(&self) -> ServiceInfo {
        ServiceInfo {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            name: self.name.clone(),
            cmd: self.cmd.clone(),
            scope: self.scope.clone(),
            ports: self.ports.clone(),
            required: self.required,
        }
    }
}

impl Store {
    /// Declare (or replace by name) a service for a project.
    pub fn set_service(
        &self,
        project_id: &str,
        name: &str,
        cmd: &str,
        scope: &str,
        ports: &[String],
        required: bool,
    ) -> Result<ServiceRow, StoreError> {
        let ports_json = serde_json::to_string(ports)?;
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO services (id, project_id, name, cmd, scope, ports, required) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(project_id, name) DO UPDATE SET \
                   cmd = excluded.cmd, scope = excluded.scope, ports = excluded.ports, \
                   required = excluded.required",
                params![
                    new_ulid().to_string(),
                    project_id,
                    name,
                    cmd,
                    scope,
                    ports_json,
                    required as i64
                ],
            )?;
        }
        self.get_service(project_id, name)?
            .ok_or_else(|| StoreError::NotFound(format!("service {name} vanished after insert")))
    }

    /// One service by `(project_id, name)`.
    pub fn get_service(&self, project_id: &str, name: &str) -> Result<Option<ServiceRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{SERVICE_SELECT} WHERE project_id = ?1 AND name = ?2"))?;
        let mut rows = stmt.query_map(params![project_id, name], service_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Every service for a project, name-sorted.
    pub fn list_services(&self, project_id: &str) -> Result<Vec<ServiceRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{SERVICE_SELECT} WHERE project_id = ?1 ORDER BY name ASC"))?;
        let rows = stmt.query_map(params![project_id], service_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Delete a service by `(project_id, name)`. Returns whether a row was removed.
    pub fn delete_service(&self, project_id: &str, name: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "DELETE FROM services WHERE project_id = ?1 AND name = ?2",
            params![project_id, name],
        )?;
        Ok(changed > 0)
    }
}

const SERVICE_SELECT: &str =
    "SELECT id, project_id, name, cmd, scope, ports, required FROM services";

fn service_from_row(row: &Row) -> rusqlite::Result<ServiceRow> {
    let ports_json: Option<String> = row.get(5)?;
    let ports = ports_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    Ok(ServiceRow {
        id: row.get(0)?,
        project_id: row.get(1)?,
        name: row.get(2)?,
        cmd: row.get(3)?,
        scope: row.get(4)?,
        ports,
        required: row.get::<_, i64>(6)? != 0,
    })
}
