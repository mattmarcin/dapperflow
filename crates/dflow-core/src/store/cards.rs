//! Card store methods (`data-model.md` / cards, card_events).
//!
//! Every card lifecycle mutation appends a `card_events` row inside the same
//! transaction and broadcasts it after commit (`tx_events`).

use rusqlite::{params, Row};

use dflow_proto::Card;

use super::{append_event, event_kind, now_ms, Store, StoreError};

/// Parameters for creating a card.
#[derive(Debug, Clone)]
pub struct NewCard {
    pub project_id: Option<String>,
    pub card_type: String,
    pub title: String,
    pub lane: String,
    pub dial_recipe: Option<String>,
    pub brief: Option<String>,
    pub priority: i64,
    pub origin_kind: String,
    pub origin_ref: Option<String>,
    /// Generic origin snapshot JSON (`data-model.md` 0007 / cards.origin_data): for a
    /// GitHub issue, the labels/url/state/assignees/milestone/number the Issue tab reads.
    pub origin_data: Option<String>,
}

impl Default for NewCard {
    fn default() -> Self {
        NewCard {
            project_id: None,
            card_type: "feature".into(),
            title: String::new(),
            lane: "inbox".into(),
            dial_recipe: None,
            brief: None,
            priority: 0,
            origin_kind: "manual".into(),
            origin_ref: None,
            origin_data: None,
        }
    }
}

/// A patch of mutable card fields; `None` leaves a field unchanged.
#[derive(Debug, Clone, Default)]
pub struct CardPatch {
    pub title: Option<String>,
    pub card_type: Option<String>,
    pub dial_recipe: Option<String>,
    pub brief: Option<String>,
    pub priority: Option<i64>,
}

/// A card query filter; `None` fields do not constrain.
#[derive(Debug, Clone, Default)]
pub struct CardQueryFilter {
    pub project_id: Option<String>,
    pub lane: Option<String>,
    pub card_type: Option<String>,
    pub limit: Option<i64>,
}

/// The outcome of an origin-keyed card upsert (`data-model.md` /
/// UNIQUE(origin_kind, origin_ref); durable audit dismissals). A dismissed finding is
/// never refiled by a re-audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginUpsert {
    /// A brand-new card was filed for this origin ref.
    Created,
    /// An existing (undismissed) card was refreshed in place rather than duplicated.
    Refreshed,
    /// An existing card the human dismissed was left untouched (refiling suppressed).
    Suppressed,
}

/// The lane a dismissed card lands in (`phase5-m3-ui.md` / Interpretation 8: bulk-triage
/// Dismiss maps to a move to Done).
pub const DISMISSED_LANE: &str = "done";

impl Store {
    /// Create a card and append its `created` event.
    pub fn create_card(&self, new: NewCard) -> Result<Card, StoreError> {
        let id = super::new_ulid().to_string();
        let ts = now_ms();
        let card = Card {
            id: id.clone(),
            project_id: new.project_id.clone(),
            card_type: new.card_type.clone(),
            title: new.title.clone(),
            lane: new.lane.clone(),
            dial_recipe: new.dial_recipe.clone(),
            priority: new.priority,
            brief: new.brief.clone(),
            origin_kind: new.origin_kind.clone(),
            origin_ref: new.origin_ref.clone(),
            created_at: ts,
            updated_at: ts,
        };
        let origin_data = new.origin_data.clone();
        self.tx_events(|tx, events| {
            tx.execute(
                "INSERT INTO cards \
                 (id, project_id, type, title, lane, dial_recipe, priority, brief, \
                  origin_kind, origin_ref, origin_data, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                params![
                    card.id,
                    card.project_id,
                    card.card_type,
                    card.title,
                    card.lane,
                    card.dial_recipe,
                    card.priority,
                    card.brief,
                    card.origin_kind,
                    card.origin_ref,
                    origin_data,
                    ts,
                ],
            )?;
            append_event(
                tx,
                events,
                &card.id,
                event_kind::CREATED,
                serde_json::json!({
                    "type": card.card_type,
                    "title": card.title,
                    "lane": card.lane,
                    "project_id": card.project_id,
                }),
            )?;
            Ok(())
        })?;
        Ok(card)
    }

    /// Fetch a card by id.
    pub fn get_card(&self, id: &str) -> Result<Option<Card>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{CARD_SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], card_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Apply a field patch to a card. A brief/title/type/priority change appends a
    /// `shaped` event; a recipe change appends `dial_changed`. Returns the new card.
    pub fn update_card(&self, id: &str, patch: CardPatch) -> Result<Card, StoreError> {
        let existing = self
            .get_card(id)?
            .ok_or_else(|| StoreError::NotFound(format!("card {id}")))?;

        let title = patch.title.clone().unwrap_or(existing.title.clone());
        let card_type = patch.card_type.clone().unwrap_or(existing.card_type.clone());
        let brief = if patch.brief.is_some() { patch.brief.clone() } else { existing.brief.clone() };
        let priority = patch.priority.unwrap_or(existing.priority);
        let dial_recipe = if patch.dial_recipe.is_some() {
            patch.dial_recipe.clone()
        } else {
            existing.dial_recipe.clone()
        };

        let content_changed = title != existing.title
            || card_type != existing.card_type
            || brief != existing.brief
            || priority != existing.priority;
        let dial_changed = dial_recipe != existing.dial_recipe;
        let ts = now_ms();

        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE cards SET title = ?2, type = ?3, brief = ?4, priority = ?5, \
                 dial_recipe = ?6, updated_at = ?7 WHERE id = ?1",
                params![id, title, card_type, brief, priority, dial_recipe, ts],
            )?;
            if content_changed {
                append_event(
                    tx,
                    events,
                    id,
                    event_kind::SHAPED,
                    serde_json::json!({ "title": title, "type": card_type, "priority": priority }),
                )?;
            }
            if dial_changed {
                append_event(
                    tx,
                    events,
                    id,
                    event_kind::DIAL_CHANGED,
                    serde_json::json!({ "from": existing.dial_recipe, "to": dial_recipe }),
                )?;
            }
            Ok(())
        })?;
        self.get_card(id)?
            .ok_or_else(|| StoreError::NotFound(format!("card {id}")))
    }

    /// Move a card to a new lane and append a `moved` event `{ from, to }`.
    ///
    /// Moving a card into the dismissed lane (`done`) stamps `dismissed_at` so an
    /// origin-carded finding's dismissal is durable, and a re-audit's fingerprint dedupe
    /// suppresses refiling it (`data-model.md` / durable audit dismissals). Moving it
    /// back out clears the stamp (un-dismiss).
    pub fn move_card(&self, id: &str, lane: &str) -> Result<Card, StoreError> {
        let existing = self
            .get_card(id)?
            .ok_or_else(|| StoreError::NotFound(format!("card {id}")))?;
        let ts = now_ms();
        let dismissed = lane == DISMISSED_LANE;
        self.tx_events(|tx, events| {
            if dismissed {
                tx.execute(
                    "UPDATE cards SET lane = ?2, dismissed_at = COALESCE(dismissed_at, ?3), \
                     updated_at = ?3 WHERE id = ?1",
                    params![id, lane, ts],
                )?;
            } else {
                tx.execute(
                    "UPDATE cards SET lane = ?2, dismissed_at = NULL, updated_at = ?3 WHERE id = ?1",
                    params![id, lane, ts],
                )?;
            }
            append_event(
                tx,
                events,
                id,
                event_kind::MOVED,
                serde_json::json!({ "from": existing.lane, "to": lane, "dismissed": dismissed }),
            )?;
            Ok(())
        })?;
        self.get_card(id)?
            .ok_or_else(|| StoreError::NotFound(format!("card {id}")))
    }

    /// Fetch a card by its origin key `(origin_kind, origin_ref)`, plus whether it is
    /// durably dismissed. `None` when no card carries that origin ref.
    pub fn get_card_by_origin(
        &self,
        origin_kind: &str,
        origin_ref: &str,
    ) -> Result<Option<(Card, bool)>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, type, title, lane, dial_recipe, priority, brief, origin_kind, \
             origin_ref, created_at, updated_at, (dismissed_at IS NOT NULL) AS dismissed \
             FROM cards WHERE origin_kind = ?1 AND origin_ref = ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![origin_kind, origin_ref], |row| {
            let card = card_from_row(row)?;
            let dismissed: i64 = row.get("dismissed")?;
            Ok((card, dismissed != 0))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// File a card by its origin ref, deduping on `(origin_kind, origin_ref)`
    /// (`data-model.md` / UNIQUE(origin_kind, origin_ref); durable audit dismissals):
    ///
    /// - no existing card -> create it (`Created`);
    /// - an existing card the human dismissed -> leave it untouched (`Suppressed`), so a
    ///   re-audit never refiles a finding already dismissed;
    /// - an existing, undismissed card -> refresh its title/brief/priority in place
    ///   (`Refreshed`), so a re-audit updates rather than duplicates.
    ///
    /// `new.origin_ref` must be set; callers without one use `create_card`.
    pub fn upsert_origin_card(&self, new: NewCard) -> Result<(Card, OriginUpsert), StoreError> {
        let origin_ref = new
            .origin_ref
            .clone()
            .ok_or_else(|| StoreError::Invalid("upsert_origin_card needs an origin_ref".into()))?;
        match self.get_card_by_origin(&new.origin_kind, &origin_ref)? {
            None => Ok((self.create_card(new)?, OriginUpsert::Created)),
            Some((card, true)) => Ok((card, OriginUpsert::Suppressed)),
            Some((existing, false)) => {
                // Re-import refreshes fields but respects local lane moves (`product.md`):
                // update_card touches title/brief/priority only, never `lane`.
                let origin_data = new.origin_data.clone();
                let card = self.update_card(
                    &existing.id,
                    CardPatch {
                        title: Some(new.title),
                        card_type: Some(new.card_type),
                        dial_recipe: None,
                        brief: new.brief.or(existing.brief),
                        priority: Some(new.priority),
                    },
                )?;
                if origin_data.is_some() {
                    self.set_card_origin_data(&existing.id, origin_data.as_deref())?;
                }
                Ok((card, OriginUpsert::Refreshed))
            }
        }
    }

    /// Set (or clear) a card's generic origin snapshot JSON (`cards.origin_data`).
    pub fn set_card_origin_data(&self, id: &str, data: Option<&str>) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute("UPDATE cards SET origin_data = ?2 WHERE id = ?1", params![id, data])?;
        Ok(())
    }

    /// Read a card's generic origin snapshot JSON, or `None` when unset.
    pub fn get_card_origin_data(&self, id: &str) -> Result<Option<String>, StoreError> {
        let conn = self.lock();
        let data: Option<String> = conn
            .query_row("SELECT origin_data FROM cards WHERE id = ?1", params![id], |r| r.get(0))
            .map_err(StoreError::from)?;
        Ok(data)
    }

    /// Query cards by an optional filter, newest first.
    pub fn query_cards(&self, filter: &CardQueryFilter) -> Result<Vec<Card>, StoreError> {
        let mut sql = String::from(CARD_SELECT);
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(pid) = &filter.project_id {
            clauses.push(format!("project_id = ?{}", args.len() + 1));
            args.push(Box::new(pid.clone()));
        }
        if let Some(lane) = &filter.lane {
            clauses.push(format!("lane = ?{}", args.len() + 1));
            args.push(Box::new(lane.clone()));
        }
        if let Some(ct) = &filter.card_type {
            clauses.push(format!("type = ?{}", args.len() + 1));
            args.push(Box::new(ct.clone()));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY id DESC");
        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows =
            stmt.query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), card_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Card events for one card, newest first, capped at `limit`. When `before` is
    /// set, only events strictly older than that cursor are returned (paging back).
    pub fn card_events(
        &self,
        card_id: &str,
        before: Option<&str>,
        limit: i64,
    ) -> Result<Vec<dflow_proto::CardEvent>, StoreError> {
        let conn = self.lock();
        let before = before.unwrap_or("~"); // '~' sorts after any Crockford base32 ULID char
        let mut stmt = conn.prepare(
            "SELECT id, card_id, kind, payload, ts FROM card_events \
             WHERE card_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3",
        )?;
        let rows =
            stmt.query_map(params![card_id, before, limit], super::card_event_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

/// Column list shared by every card read, so `card_from_row` indexing stays stable.
const CARD_SELECT: &str = "SELECT id, project_id, type, title, lane, dial_recipe, priority, \
     brief, origin_kind, origin_ref, created_at, updated_at FROM cards";

/// Map a `cards` row (in `CARD_SELECT` order) to the wire `Card`.
fn card_from_row(row: &Row) -> rusqlite::Result<Card> {
    Ok(Card {
        id: row.get(0)?,
        project_id: row.get(1)?,
        card_type: row.get(2)?,
        title: row.get(3)?,
        lane: row.get(4)?,
        dial_recipe: row.get(5)?,
        priority: row.get(6)?,
        brief: row.get(7)?,
        origin_kind: row.get(8)?,
        origin_ref: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn audit_card(project_id: &str, title: &str, fingerprint: &str) -> NewCard {
        NewCard {
            project_id: Some(project_id.to_string()),
            title: title.to_string(),
            origin_kind: "audit".into(),
            origin_ref: Some(fingerprint.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn origin_upsert_creates_then_refreshes_then_suppresses_on_dismissal() {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project("/tmp/audit", "audit", "main", "pr").unwrap();

        // First filing of a fingerprint creates the card.
        let (card, outcome) = store.upsert_origin_card(audit_card(&project.id, "N+1 query in users", "fp-users")).unwrap();
        assert_eq!(outcome, OriginUpsert::Created);
        assert_eq!(card.origin_ref.as_deref(), Some("fp-users"));

        // A re-audit with the same fingerprint refreshes in place, never duplicates.
        let (refreshed, outcome) = store
            .upsert_origin_card(audit_card(&project.id, "N+1 query in users (again)", "fp-users"))
            .unwrap();
        assert_eq!(outcome, OriginUpsert::Refreshed);
        assert_eq!(refreshed.id, card.id, "refresh targets the same card");
        assert_eq!(refreshed.title, "N+1 query in users (again)");
        // Still exactly one card for this project.
        assert_eq!(store.query_cards(&CardQueryFilter { project_id: Some(project.id.clone()), ..Default::default() }).unwrap().len(), 1);

        // The human dismisses it (moves it to Done); dismissal is durable.
        store.move_card(&card.id, DISMISSED_LANE).unwrap();
        let (_c, dismissed) = store.get_card_by_origin("audit", "fp-users").unwrap().unwrap();
        assert!(dismissed, "moving to the dismissed lane stamps dismissed_at");

        // A later re-audit does NOT refile the dismissed finding.
        let (suppressed, outcome) = store
            .upsert_origin_card(audit_card(&project.id, "N+1 query in users", "fp-users"))
            .unwrap();
        assert_eq!(outcome, OriginUpsert::Suppressed);
        assert_eq!(suppressed.id, card.id);
        assert_eq!(suppressed.lane, DISMISSED_LANE, "the dismissed card is left untouched");
        assert_eq!(store.query_cards(&CardQueryFilter { project_id: Some(project.id.clone()), ..Default::default() }).unwrap().len(), 1);

        // Moving it back out of Done un-dismisses it, so a re-audit refreshes again.
        store.move_card(&card.id, "inbox").unwrap();
        let (_c, dismissed) = store.get_card_by_origin("audit", "fp-users").unwrap().unwrap();
        assert!(!dismissed, "moving out of the dismissed lane clears dismissed_at");
        let (_c, outcome) = store.upsert_origin_card(audit_card(&project.id, "N+1", "fp-users")).unwrap();
        assert_eq!(outcome, OriginUpsert::Refreshed);
    }
}
