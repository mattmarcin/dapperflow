//! Session store methods (`data-model.md` / sessions) and daemon-restart
//! reconciliation (`architecture.md` / Daemon restarts and session resume).
//!
//! The live session manager persists state transitions here; on startup,
//! reconciliation marks any still-running row `interrupted` (never deleted).

use rusqlite::{params, Row};

use super::needs_you::{raise_needs_you_tx, resolve_needs_you_tx};
use super::{append_event, event_kind, now_ms, session_state, Store, StoreError};
use crate::store::models::SessionRow;

/// Parameters for persisting a newly spawned session. `id` is the live session's
/// ULID, so the DB row and the in-memory `Session` share one identity.
#[derive(Debug, Clone, Default)]
pub struct NewSession {
    pub id: String,
    /// Nullable since session-first: cardless (bare `session.create`) sessions are
    /// legitimate and persist too (`data-model.md`).
    pub card_id: Option<String>,
    /// cwd->project match captured at create, for cardless sessions (Phase 2).
    pub project_id: Option<String>,
    /// Working directory, persisted for resume (Phase 2).
    pub cwd: Option<String>,
    pub harness: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub state: String,
    pub worktree_id: Option<String>,
    pub scrollback_path: String,
    pub first_prompt: Option<String>,
    pub resumed_from: Option<String>,
    pub title: Option<String>,
    /// The configured launcher this session was dispatched through (`agents.id`),
    /// when one was resolved (Phase 1.5).
    pub agent_id: Option<String>,
}

impl Store {
    /// Insert a session row and, for a carded session, append its `session_started`
    /// event. A cardless session has no card timeline, so no event is appended
    /// (`card_events.card_id` is NOT NULL); its row still persists for reconciliation.
    pub fn create_session(&self, new: NewSession) -> Result<SessionRow, StoreError> {
        let ts = now_ms();
        self.tx_events(|tx, events| {
            tx.execute(
                "INSERT INTO sessions \
                 (id, card_id, project_id, cwd, harness, model, effort, state, worktree_id, \
                  scrollback_path, resume_ref, resumed_from, first_prompt, title, agent_id, \
                  created_at, ended_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?12, ?13, ?14, ?15, NULL)",
                params![
                    new.id,
                    new.card_id,
                    new.project_id,
                    new.cwd,
                    new.harness,
                    new.model,
                    new.effort,
                    new.state,
                    new.worktree_id,
                    new.scrollback_path,
                    new.resumed_from,
                    new.first_prompt,
                    new.title,
                    new.agent_id,
                    ts,
                ],
            )?;
            if let Some(card_id) = &new.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::SESSION_STARTED,
                    serde_json::json!({
                        "session_id": new.id,
                        "harness": new.harness,
                        "worktree_id": new.worktree_id,
                        "state": new.state,
                        "resumed_from": new.resumed_from,
                    }),
                )?;
            }
            Ok(())
        })?;
        self.get_session(&new.id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {} vanished after insert", new.id)))
    }

    /// Fetch a session row by id.
    pub fn get_session(&self, id: &str) -> Result<Option<SessionRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{SESSION_SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], session_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// All session rows for one card, newest first.
    pub fn card_session_rows(&self, card_id: &str) -> Result<Vec<SessionRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{SESSION_SELECT} WHERE card_id = ?1 ORDER BY id DESC"))?;
        let rows = stmt.query_map(params![card_id], session_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Count sessions that have not ended (`ended_at IS NULL`): the live-session count
    /// for `dflowd --status`.
    pub fn count_live_sessions(&self) -> Result<i64, StoreError> {
        let conn = self.lock();
        let n: i64 =
            conn.query_row("SELECT count(*) FROM sessions WHERE ended_at IS NULL", [], |r| r.get(0))?;
        Ok(n)
    }

    /// Every session row, newest first (session.list enrichment, reconciliation).
    pub fn all_session_rows(&self) -> Result<Vec<SessionRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{SESSION_SELECT} ORDER BY id DESC"))?;
        let rows = stmt.query_map([], session_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Transition a session to `new_state`, appending a `state_changed` event.
    /// Terminal states also stamp `ended_at`. A no-op transition still records the
    /// event so the timeline reflects the observation; callers debounce upstream.
    pub fn set_session_state(&self, id: &str, new_state: &str) -> Result<(), StoreError> {
        self.set_session_state_note(id, new_state, None)
    }

    /// Like [`set_session_state`] but with an optional status note recorded on the
    /// session row (the board session strip reads it) and in the `state_changed`
    /// payload (`agent-cli.md` / `dflow status <state> [note]`; the note "lands as an
    /// event and updates the session status_note the UI subtitles read").
    pub fn set_session_state_note(
        &self,
        id: &str,
        new_state: &str,
        note: Option<&str>,
    ) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let ended = session_state::is_terminal(new_state);
        let ts = now_ms();
        self.tx_events(|tx, events| {
            match (ended, note) {
                (true, Some(n)) => tx.execute(
                    "UPDATE sessions SET state = ?2, status_note = ?4, ended_at = COALESCE(ended_at, ?3) WHERE id = ?1",
                    params![id, new_state, ts, n],
                )?,
                (true, None) => tx.execute(
                    "UPDATE sessions SET state = ?2, ended_at = COALESCE(ended_at, ?3) WHERE id = ?1",
                    params![id, new_state, ts],
                )?,
                (false, Some(n)) => tx.execute(
                    "UPDATE sessions SET state = ?2, status_note = ?3 WHERE id = ?1",
                    params![id, new_state, n],
                )?,
                (false, None) => {
                    tx.execute("UPDATE sessions SET state = ?2 WHERE id = ?1", params![id, new_state])?
                }
            };
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::STATE_CHANGED,
                    serde_json::json!({ "session_id": id, "from": row.state, "to": new_state, "note": note }),
                )?;
            }
            Ok(())
        })
    }

    /// Set the session-strip status note without a state change (`dflow card note`).
    /// The note lands on the session row and, for a carded session, as a
    /// `state_changed` event (from == to) so the timeline shows what the agent said.
    /// Returns whether a session row matched.
    pub fn set_session_status_note(&self, id: &str, note: &str) -> Result<bool, StoreError> {
        let row = match self.get_session(id)? {
            Some(row) => row,
            None => return Ok(false),
        };
        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE sessions SET status_note = ?2 WHERE id = ?1",
                params![id, note],
            )?;
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::STATE_CHANGED,
                    serde_json::json!({ "session_id": id, "from": row.state, "to": row.state, "note": note }),
                )?;
            }
            Ok(())
        })?;
        Ok(true)
    }

    /// Mark a session `needs_input`: append `needs_input`, raise a Needs You item.
    /// One atomic transaction so the projection and the event never diverge.
    ///
    /// `kind` distinguishes the Needs You reason (`data-model.md` /
    /// needs_you_items.kind): `trust_dialog` for a trust/permission gate, `agent_blocked`
    /// for an agent stuck awaiting a human decision (Phase 2, tier-2 enrichment).
    /// A cardless session has no card to hang a Needs You item on, so it transitions
    /// state only (no event, no Needs You item), which is honest: nothing in the board
    /// is waiting on it.
    pub fn mark_session_needs_input(
        &self,
        id: &str,
        kind: &str,
        score: i64,
    ) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let dedupe_key = format!("needs_input:{id}");
        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE sessions SET state = ?2 WHERE id = ?1",
                params![id, session_state::NEEDS_INPUT],
            )?;
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::NEEDS_INPUT,
                    serde_json::json!({ "session_id": id, "kind": kind }),
                )?;
                raise_needs_you_tx(tx, events, card_id, kind, &dedupe_key, score)?;
            }
            Ok(())
        })
    }

    /// Clear a `needs_input` session back to `new_state`, resolving its Needs You item
    /// and appending `state_changed` (resolution on next input).
    pub fn clear_session_needs_input(
        &self,
        id: &str,
        new_state: &str,
    ) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let dedupe_key = format!("needs_input:{id}");
        self.tx_events(|tx, events| {
            tx.execute("UPDATE sessions SET state = ?2 WHERE id = ?1", params![id, new_state])?;
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::STATE_CHANGED,
                    serde_json::json!({ "session_id": id, "from": row.state, "to": new_state }),
                )?;
                resolve_needs_you_tx(tx, events, card_id, &dedupe_key, "ui")?;
            }
            Ok(())
        })
    }

    /// Tier-1 `dflow status working [note]`: move to `working`, set the status note,
    /// and resolve any open attention item for this session (the agent is unstuck).
    /// One atomic transaction (`agent-cli.md` / tier-1 self-report).
    pub fn agent_report_working(&self, id: &str, note: Option<&str>) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let dedupe_key = format!("needs_input:{id}");
        self.tx_events(|tx, events| {
            match note {
                Some(n) => tx.execute(
                    "UPDATE sessions SET state = ?2, status_note = ?3 WHERE id = ?1",
                    params![id, session_state::WORKING, n],
                )?,
                None => tx.execute(
                    "UPDATE sessions SET state = ?2 WHERE id = ?1",
                    params![id, session_state::WORKING],
                )?,
            };
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::STATE_CHANGED,
                    serde_json::json!({ "session_id": id, "from": row.state, "to": session_state::WORKING, "note": note }),
                )?;
                resolve_needs_you_tx(tx, events, card_id, &dedupe_key, "agent")?;
            }
            Ok(())
        })
    }

    /// Tier-1 `dflow status blocked <note>`: move to `blocked`, record the note, append
    /// a `blocked` event, and raise a Needs You item. Shares the `needs_input:{id}`
    /// dedupe key with tier-2/3 so a later `working` resolves it. One atomic tx.
    pub fn agent_report_blocked(&self, id: &str, note: &str, score: i64) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let dedupe_key = format!("needs_input:{id}");
        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE sessions SET state = ?2, status_note = ?3 WHERE id = ?1",
                params![id, session_state::BLOCKED, note],
            )?;
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::BLOCKED,
                    serde_json::json!({ "session_id": id, "note": note }),
                )?;
                raise_needs_you_tx(tx, events, card_id, "agent_blocked", &dedupe_key, score)?;
            }
            Ok(())
        })
    }

    /// Tier-1 `dflow status done [note]`: move to the terminal `done` state (stamping
    /// `ended_at`), record the note, append `session_ended`, and resolve any open
    /// attention item. The daemon arbitrates whether `done` is granted before calling
    /// this (`agent-cli.md` / Stage advancement arbitration). One atomic tx.
    pub fn agent_report_done(&self, id: &str, note: Option<&str>) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let dedupe_key = format!("needs_input:{id}");
        let ts = now_ms();
        self.tx_events(|tx, events| {
            match note {
                Some(n) => tx.execute(
                    "UPDATE sessions SET state = ?2, status_note = ?3, ended_at = COALESCE(ended_at, ?4) WHERE id = ?1",
                    params![id, session_state::DONE, n, ts],
                )?,
                None => tx.execute(
                    "UPDATE sessions SET state = ?2, ended_at = COALESCE(ended_at, ?3) WHERE id = ?1",
                    params![id, session_state::DONE, ts],
                )?,
            };
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::SESSION_ENDED,
                    serde_json::json!({ "session_id": id, "state": session_state::DONE, "from": row.state, "note": note }),
                )?;
                resolve_needs_you_tx(tx, events, card_id, &dedupe_key, "agent")?;
            }
            Ok(())
        })
    }

    /// Finalize a session that ended during this run: set `state`, stamp `ended_at`,
    /// append `session_ended`.
    pub fn finalize_session(&self, id: &str, state: &str) -> Result<(), StoreError> {
        let row = self
            .get_session(id)?
            .ok_or_else(|| StoreError::NotFound(format!("session {id}")))?;
        let ts = now_ms();
        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE sessions SET state = ?2, ended_at = COALESCE(ended_at, ?3) WHERE id = ?1",
                params![id, state, ts],
            )?;
            if let Some(card_id) = &row.card_id {
                append_event(
                    tx,
                    events,
                    card_id,
                    event_kind::SESSION_ENDED,
                    serde_json::json!({ "session_id": id, "state": state, "from": row.state }),
                )?;
            }
            Ok(())
        })
    }

    /// Set (or clear, on empty) a session's tab title. Returns whether a row matched.
    pub fn set_session_title(&self, id: &str, title: &str) -> Result<bool, StoreError> {
        let value: Option<&str> = if title.is_empty() { None } else { Some(title) };
        let conn = self.lock();
        let changed =
            conn.execute("UPDATE sessions SET title = ?2 WHERE id = ?1", params![id, value])?;
        Ok(changed > 0)
    }

    /// Persist a captured harness-native resume ref (`adapters.md`). Phase 2 wiring.
    pub fn set_resume_ref(&self, id: &str, resume_ref: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE sessions SET resume_ref = ?2 WHERE id = ?1",
            params![id, resume_ref],
        )?;
        Ok(changed > 0)
    }

    /// Daemon-startup reconciliation: every non-terminal session becomes
    /// `interrupted` (its PTY died with the daemon), each with a `state_changed`
    /// event. Returns the ids marked. Never deletes a row.
    pub fn reconcile_interrupted(&self) -> Result<Vec<String>, StoreError> {
        let stale: Vec<SessionRow> = self
            .all_session_rows()?
            .into_iter()
            .filter(|s| !session_state::is_terminal(&s.state))
            .collect();
        if stale.is_empty() {
            return Ok(Vec::new());
        }
        let ts = now_ms();
        let ids: Vec<String> = stale.iter().map(|s| s.id.clone()).collect();
        self.tx_events(|tx, events| {
            for s in &stale {
                tx.execute(
                    "UPDATE sessions SET state = ?2, ended_at = COALESCE(ended_at, ?3) WHERE id = ?1",
                    params![s.id, session_state::INTERRUPTED, ts],
                )?;
                if let Some(card_id) = &s.card_id {
                    append_event(
                        tx,
                        events,
                        card_id,
                        event_kind::STATE_CHANGED,
                        serde_json::json!({
                            "session_id": s.id,
                            "from": s.state,
                            "to": session_state::INTERRUPTED,
                            "reason": "daemon_restart",
                        }),
                    )?;
                }
            }
            Ok(())
        })?;
        Ok(ids)
    }
}

const SESSION_SELECT: &str = "SELECT id, card_id, project_id, cwd, harness, model, effort, state, \
     worktree_id, scrollback_path, resume_ref, resumed_from, first_prompt, title, status_note, \
     agent_id, created_at, ended_at FROM sessions";

fn session_from_row(row: &Row) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        project_id: row.get(2)?,
        cwd: row.get(3)?,
        harness: row.get(4)?,
        model: row.get(5)?,
        effort: row.get(6)?,
        state: row.get(7)?,
        worktree_id: row.get(8)?,
        scrollback_path: row.get(9)?,
        resume_ref: row.get(10)?,
        resumed_from: row.get(11)?,
        first_prompt: row.get(12)?,
        title: row.get(13)?,
        status_note: row.get(14)?,
        agent_id: row.get(15)?,
        created_at: row.get(16)?,
        ended_at: row.get(17)?,
    })
}
