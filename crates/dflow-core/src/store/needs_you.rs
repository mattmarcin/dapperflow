//! Needs You projection store methods (`data-model.md` / needs_you_items).
//!
//! The table is a persisted projection kept in lockstep with the
//! `needs_you_raised` / `needs_you_resolved` events (`data-model.md`). Phase 1
//! populates it for the needs-input heuristic; richer kinds arrive in later phases.

use rusqlite::{params, Row, Transaction};

use dflow_proto::CardEvent;

use super::{append_event, event_kind, now_ms, NeedsYouItem, Store, StoreError};

impl Store {
    /// Raise (or re-raise) a Needs You item and append `needs_you_raised`.
    ///
    /// Idempotent on `(card_id, dedupe_key)`: an existing item is updated in place
    /// (re-opened if it was resolved) rather than duplicated.
    pub fn raise_needs_you(
        &self,
        card_id: &str,
        kind: &str,
        dedupe_key: &str,
        score: i64,
    ) -> Result<NeedsYouItem, StoreError> {
        self.tx_events(|tx, events| {
            raise_needs_you_tx(tx, events, card_id, kind, dedupe_key, score)
        })
    }

    /// Resolve a Needs You item and append `needs_you_resolved`. Returns the resolved
    /// item, or `None` if there was no open item for that key.
    pub fn resolve_needs_you(
        &self,
        card_id: &str,
        dedupe_key: &str,
        resolved_by: &str,
    ) -> Result<Option<NeedsYouItem>, StoreError> {
        self.tx_events(|tx, events| resolve_needs_you_tx(tx, events, card_id, dedupe_key, resolved_by))
    }

    /// List Needs You items; `open_only` filters to unresolved ones, highest score first.
    pub fn list_needs_you(&self, open_only: bool) -> Result<Vec<NeedsYouItem>, StoreError> {
        let conn = self.lock();
        let sql = if open_only {
            format!("{NEEDS_YOU_SELECT} WHERE resolved_at IS NULL ORDER BY score DESC, raised_at ASC")
        } else {
            format!("{NEEDS_YOU_SELECT} ORDER BY score DESC, raised_at ASC")
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], needs_you_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Open Needs You items that have not yet been notified (`notified_at IS NULL`),
    /// highest score first: the feed a future notification throttle drains
    /// (`data-model.md` / needs_you_items.notified_at). One notification per item per
    /// quiet period, so a notified item is not re-surfaced until re-raised.
    pub fn list_unnotified_needs_you(&self) -> Result<Vec<NeedsYouItem>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "{NEEDS_YOU_SELECT} WHERE resolved_at IS NULL AND notified_at IS NULL \
             ORDER BY score DESC, raised_at ASC"
        ))?;
        let rows = stmt.query_map([], needs_you_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Stamp `notified_at` on a Needs You item so the throttle does not re-notify it
    /// until it is re-raised (re-raising clears the stamp). Returns whether a row matched.
    pub fn mark_needs_you_notified(&self, card_id: &str, dedupe_key: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE needs_you_items SET notified_at = ?3 \
             WHERE card_id = ?1 AND dedupe_key = ?2 AND resolved_at IS NULL",
            params![card_id, dedupe_key, now_ms()],
        )?;
        Ok(changed > 0)
    }
}

/// Upsert a Needs You item inside a transaction and append its raised event.
pub(super) fn raise_needs_you_tx(
    tx: &Transaction,
    events: &mut Vec<CardEvent>,
    card_id: &str,
    kind: &str,
    dedupe_key: &str,
    score: i64,
) -> Result<NeedsYouItem, StoreError> {
    let id = super::new_ulid().to_string();
    let now = now_ms();
    tx.execute(
        "INSERT INTO needs_you_items (id, card_id, kind, dedupe_key, score, raised_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(card_id, dedupe_key) DO UPDATE SET \
           kind = excluded.kind, score = excluded.score, raised_at = excluded.raised_at, \
           resolved_at = NULL, resolved_by = NULL, notified_at = NULL",
        params![id, card_id, kind, dedupe_key, score, now],
    )?;
    append_event(
        tx,
        events,
        card_id,
        event_kind::NEEDS_YOU_RAISED,
        serde_json::json!({ "kind": kind, "dedupe_key": dedupe_key, "score": score }),
    )?;
    let mut stmt = tx.prepare(&format!(
        "{NEEDS_YOU_SELECT} WHERE card_id = ?1 AND dedupe_key = ?2"
    ))?;
    let item = stmt.query_row(params![card_id, dedupe_key], needs_you_from_row)?;
    Ok(item)
}

/// Resolve an open Needs You item inside a transaction and append its resolved event.
pub(super) fn resolve_needs_you_tx(
    tx: &Transaction,
    events: &mut Vec<CardEvent>,
    card_id: &str,
    dedupe_key: &str,
    resolved_by: &str,
) -> Result<Option<NeedsYouItem>, StoreError> {
    let now = now_ms();
    let changed = tx.execute(
        "UPDATE needs_you_items SET resolved_at = ?3, resolved_by = ?4 \
         WHERE card_id = ?1 AND dedupe_key = ?2 AND resolved_at IS NULL",
        params![card_id, dedupe_key, now, resolved_by],
    )?;
    if changed == 0 {
        return Ok(None);
    }
    append_event(
        tx,
        events,
        card_id,
        event_kind::NEEDS_YOU_RESOLVED,
        serde_json::json!({ "dedupe_key": dedupe_key, "resolved_by": resolved_by }),
    )?;
    let mut stmt = tx.prepare(&format!(
        "{NEEDS_YOU_SELECT} WHERE card_id = ?1 AND dedupe_key = ?2"
    ))?;
    let item = stmt.query_row(params![card_id, dedupe_key], needs_you_from_row)?;
    Ok(Some(item))
}

const NEEDS_YOU_SELECT: &str = "SELECT id, card_id, kind, dedupe_key, score, raised_at, \
     notified_at, resolved_at, resolved_by FROM needs_you_items";

fn needs_you_from_row(row: &Row) -> rusqlite::Result<NeedsYouItem> {
    Ok(NeedsYouItem {
        id: row.get(0)?,
        card_id: row.get(1)?,
        kind: row.get(2)?,
        dedupe_key: row.get(3)?,
        score: row.get(4)?,
        raised_at: row.get(5)?,
        notified_at: row.get(6)?,
        resolved_at: row.get(7)?,
        resolved_by: row.get(8)?,
    })
}
