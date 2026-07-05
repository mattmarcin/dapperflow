//! Configured-launcher store methods (`data-model.md` / agents, `product.md` /
//! Settings > Agents).
//!
//! Launchers are user data, not card-scoped, so they mutate in place and append no
//! `card_events` (`data-model.md` / Honesty note). The wire `Agent` carries a
//! computed `caution` flag (`crate::agents::caution`); the stored row does not.

use std::collections::BTreeMap;

use rusqlite::{params, Row};

use dflow_proto::Agent;

use super::{Store, StoreError};
use crate::agents::{caution, DetectedCli};

/// `agents.source` values (`data-model.md` / agents.source).
pub mod agent_source {
    /// Created by a PATH scan (`agents.detect`).
    pub const DETECTED: &str = "detected";
    /// Added by the user (`agents.add`).
    pub const CUSTOM: &str = "custom";
}

/// Parameters for inserting a launcher.
#[derive(Debug, Clone)]
pub struct NewAgent {
    pub name: String,
    pub adapter: String,
    pub command: String,
    pub extra_args: Vec<String>,
    pub extra_env: BTreeMap<String, String>,
    /// `detected` | `custom` (`agent_source`).
    pub source: String,
    pub detected_version: Option<String>,
    pub enabled: bool,
}

/// A patch of mutable launcher fields; `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct AgentPatch {
    pub name: Option<String>,
    pub adapter: Option<String>,
    pub command: Option<String>,
    pub extra_args: Option<Vec<String>>,
    pub extra_env: Option<BTreeMap<String, String>>,
    pub enabled: Option<bool>,
}

/// The outcome of one `agents.detect` run.
#[derive(Debug, Clone)]
pub struct DetectionOutcome {
    /// Every CLI the scan found, in catalog order, tagged with whether this run
    /// created a brand-new launcher for it.
    pub found: Vec<(DetectedCli, bool)>,
}

impl Store {
    /// Insert a launcher. The unique `name` constraint rejects a duplicate.
    pub fn insert_agent(&self, new: NewAgent) -> Result<Agent, StoreError> {
        let id = super::new_ulid().to_string();
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO agents \
                 (id, name, adapter, command, extra_args, extra_env, source, detected_version, enabled) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    id,
                    new.name,
                    new.adapter,
                    new.command,
                    args_to_json(&new.extra_args),
                    env_to_json(&new.extra_env),
                    new.source,
                    new.detected_version,
                    new.enabled as i64,
                ],
            )?;
        }
        self.get_agent(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("agent {id} vanished after insert")))
    }

    /// Fetch a launcher by id.
    pub fn get_agent(&self, id: &str) -> Result<Option<Agent>, StoreError> {
        self.query_agent("WHERE id = ?1", params![id])
    }

    /// Fetch a launcher by its unique name.
    pub fn get_agent_by_name(&self, name: &str) -> Result<Option<Agent>, StoreError> {
        self.query_agent("WHERE name = ?1", params![name])
    }

    /// Resolve a launcher reference that is either an id or a name (id wins). Used by
    /// dispatch (`agent` param) and by `agents.update` / `agents.remove`.
    pub fn resolve_agent(&self, reference: &str) -> Result<Option<Agent>, StoreError> {
        if let Some(agent) = self.get_agent(reference)? {
            return Ok(Some(agent));
        }
        self.get_agent_by_name(reference)
    }

    /// All launchers, newest first.
    pub fn list_agents(&self) -> Result<Vec<Agent>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{AGENT_SELECT} ORDER BY id DESC"))?;
        let rows = stmt.query_map([], agent_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Apply a field patch to a launcher (`None` leaves a field unchanged).
    pub fn update_agent(&self, id: &str, patch: AgentPatch) -> Result<Agent, StoreError> {
        let existing = self
            .get_agent(id)?
            .ok_or_else(|| StoreError::NotFound(format!("agent {id}")))?;
        let name = patch.name.unwrap_or(existing.name);
        let adapter = patch.adapter.unwrap_or(existing.adapter);
        let command = patch.command.unwrap_or(existing.command);
        let extra_args = patch.extra_args.unwrap_or(existing.extra_args);
        let extra_env = patch.extra_env.unwrap_or(existing.extra_env);
        let enabled = patch.enabled.unwrap_or(existing.enabled);
        {
            let conn = self.lock();
            conn.execute(
                "UPDATE agents SET name = ?2, adapter = ?3, command = ?4, extra_args = ?5, \
                 extra_env = ?6, enabled = ?7 WHERE id = ?1",
                params![
                    id,
                    name,
                    adapter,
                    command,
                    args_to_json(&extra_args),
                    env_to_json(&extra_env),
                    enabled as i64,
                ],
            )?;
        }
        self.get_agent(id)?
            .ok_or_else(|| StoreError::NotFound(format!("agent {id}")))
    }

    /// Toggle a launcher's `enabled` flag (`data-model.md` / agents.enabled).
    pub fn set_agent_enabled(&self, id: &str, enabled: bool) -> Result<Agent, StoreError> {
        {
            let conn = self.lock();
            let changed = conn.execute(
                "UPDATE agents SET enabled = ?2 WHERE id = ?1",
                params![id, enabled as i64],
            )?;
            if changed == 0 {
                return Err(StoreError::NotFound(format!("agent {id}")));
            }
        }
        self.get_agent(id)?
            .ok_or_else(|| StoreError::NotFound(format!("agent {id}")))
    }

    /// Remove a launcher, refusing while any non-ended session references it: a live
    /// session's harness behavior is bound to this launcher, so pulling it out from
    /// under the session would strand it. The error suggests disabling instead
    /// (`product.md` / Settings > Agents). Returns the removed launcher.
    ///
    /// Ended sessions may still reference the launcher via `agent_id`; those links are
    /// detached (set null) in the same transaction so the foreign key permits the
    /// delete. The launcher is gone either way; each session keeps its recorded
    /// `harness` (adapter family), losing only the specific launcher id.
    pub fn remove_agent(&self, id: &str) -> Result<Agent, StoreError> {
        let agent = self
            .get_agent(id)?
            .ok_or_else(|| StoreError::NotFound(format!("agent {id}")))?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let active: i64 = tx.query_row(
            "SELECT count(*) FROM sessions WHERE agent_id = ?1 AND ended_at IS NULL",
            params![id],
            |r| r.get(0),
        )?;
        if active > 0 {
            // tx drops here without commit -> rollback, nothing changed.
            return Err(StoreError::Invalid(format!(
                "launcher '{}' is in use by {active} active session(s); disable it instead of removing",
                agent.name
            )));
        }
        tx.execute("UPDATE sessions SET agent_id = NULL WHERE agent_id = ?1", params![id])?;
        tx.execute("DELETE FROM agents WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(agent)
    }

    /// Upsert detected CLIs into launchers (`product.md` / Autodetection).
    ///
    /// For each found CLI: create a new enabled `source: detected` launcher if none
    /// with that name exists; refresh `command` and `detected_version` on an existing
    /// `detected` row while never touching its user-editable fields
    /// (`extra_args` / `extra_env` / `name` / `enabled`); leave `custom` rows entirely
    /// alone. Detection never disables or removes launchers for CLIs it did not find.
    pub fn apply_detection(&self, found: Vec<DetectedCli>) -> Result<DetectionOutcome, StoreError> {
        let found_cursor = found.iter().any(|c| c.name == "cursor");
        let mut outcome = Vec::with_capacity(found.len());
        for cli in found {
            let created = match self.get_agent_by_name(&cli.name)? {
                None => {
                    self.insert_agent(NewAgent {
                        name: cli.name.clone(),
                        adapter: cli.adapter.clone(),
                        command: cli.command.clone(),
                        extra_args: Vec::new(),
                        extra_env: BTreeMap::new(),
                        source: agent_source::DETECTED.to_string(),
                        detected_version: cli.version.clone(),
                        // A cursor launcher whose command still resolves to the desktop
                        // editor shim must never be enabled to launch the GUI; probing
                        // cursor-agent means a fresh cursor row already carries the CLI
                        // command, but guard anyway.
                        enabled: !crate::agents::is_cursor_editor_shim(&cli.command),
                    })?;
                    true
                }
                Some(existing) if existing.source == agent_source::DETECTED => {
                    // Refresh only the detection-owned fields; user edits are preserved.
                    // For cursor this repoints a stale editor-shim command at the
                    // cursor-agent CLI and re-enables it (Phase 2 correction).
                    let conn = self.lock();
                    conn.execute(
                        "UPDATE agents SET command = ?2, detected_version = ?3, enabled = 1 \
                         WHERE id = ?1",
                        params![existing.id, cli.command, cli.version],
                    )?;
                    false
                }
                Some(_custom) => false, // custom rows are never touched by detection
            };
            outcome.push((cli, created));
        }

        // Cursor correction (Phase 2): if cursor-agent was NOT found this run but a
        // stale detected `cursor` launcher still points at the desktop editor shim,
        // disable it so it can never open the GUI. Never touches a custom launcher.
        if !found_cursor {
            if let Some(existing) = self.get_agent_by_name("cursor")? {
                if existing.source == agent_source::DETECTED
                    && existing.enabled
                    && crate::agents::is_cursor_editor_shim(&existing.command)
                {
                    let conn = self.lock();
                    conn.execute(
                        "UPDATE agents SET enabled = 0 WHERE id = ?1",
                        params![existing.id],
                    )?;
                }
            }
        }

        Ok(DetectionOutcome { found: outcome })
    }

    fn query_agent(
        &self,
        where_clause: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<Agent>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{AGENT_SELECT} {where_clause}"))?;
        let mut rows = stmt.query_map(params, agent_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }
}

const AGENT_SELECT: &str = "SELECT id, name, adapter, command, extra_args, extra_env, source, \
     detected_version, enabled FROM agents";

/// Map an `agents` row to the wire `Agent`, computing `caution` from `extra_args`.
fn agent_from_row(row: &Row) -> rusqlite::Result<Agent> {
    let extra_args = args_from_json(row.get::<_, Option<String>>(4)?.as_deref());
    let extra_env = env_from_json(row.get::<_, Option<String>>(5)?.as_deref());
    let caution = caution(&extra_args);
    Ok(Agent {
        id: row.get(0)?,
        name: row.get(1)?,
        adapter: row.get(2)?,
        command: row.get(3)?,
        extra_args,
        extra_env,
        source: row.get(6)?,
        detected_version: row.get(7)?,
        enabled: row.get::<_, i64>(8)? != 0,
        caution,
    })
}

/// Serialize `extra_args` to the JSON stored in `agents.extra_args`; empty -> NULL.
fn args_to_json(args: &[String]) -> Option<String> {
    if args.is_empty() {
        None
    } else {
        serde_json::to_string(args).ok()
    }
}

/// Serialize `extra_env` to the JSON stored in `agents.extra_env`; empty -> NULL.
fn env_to_json(env: &BTreeMap<String, String>) -> Option<String> {
    if env.is_empty() {
        None
    } else {
        serde_json::to_string(env).ok()
    }
}

fn args_from_json(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()).unwrap_or_default()
}

fn env_from_json(raw: Option<&str>) -> BTreeMap<String, String> {
    raw.and_then(|s| serde_json::from_str::<BTreeMap<String, String>>(s).ok()).unwrap_or_default()
}
