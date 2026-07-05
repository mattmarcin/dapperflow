//! Plan Studio artifact + annotation store methods (`data-model.md` / artifacts,
//! annotations; `plan-studio.md`).
//!
//! An artifact is a plan/mockup/diagram HTML the agent registers for review. It is
//! keyed by `(card_id, path)` so re-registering the same path is a revision (bumped
//! round, fresh `revised_doc_id`, stable `doc_id` so the served URL identity is stable).
//! Annotations are the human's feedback items, stored per round with a `queued | sent`
//! delivery state so a batch is never lost: it persists until a `dflow plan poll`
//! consumes it (`plan-studio.md`: "Feedback is never lost").

use rusqlite::{params, Row};

use dflow_proto::{ArtifactMeta, FeedbackItem};

use super::{new_ulid, now_ms, Store, StoreError};

/// `artifacts.status` values (`data-model.md` / artifacts).
pub mod artifact_status {
    pub const OPEN: &str = "open";
    pub const AWAITING_FEEDBACK: &str = "awaiting_feedback";
    pub const APPROVED: &str = "approved";
    pub const ENDED: &str = "ended";

    /// Whether the review has concluded (no more feedback rounds).
    pub fn is_ended(status: &str) -> bool {
        matches!(status, APPROVED | ENDED)
    }
}

/// A row of the `artifacts` table.
#[derive(Debug, Clone)]
pub struct ArtifactRow {
    pub id: String,
    pub card_id: String,
    pub path: String,
    pub kind: String,
    pub title: Option<String>,
    pub doc_id: String,
    pub revised_doc_id: Option<String>,
    pub round: i64,
    /// JSON layout-audit result (the latest `layout_warnings`).
    pub audit: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ArtifactRow {
    /// The wire `ArtifactMeta` (`phase5-m3-ui.md` / Interpretation 2).
    pub fn to_meta(&self) -> ArtifactMeta {
        ArtifactMeta {
            id: self.id.clone(),
            card_id: self.card_id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            doc_id: self.doc_id.clone(),
            revised_doc_id: self.revised_doc_id.clone(),
            round: self.round.max(0) as u32,
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    /// The stored layout audit parsed into `layout_warnings`, or empty.
    pub fn layout_warnings(&self) -> Vec<dflow_proto::LayoutWarning> {
        self.audit
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }
}

impl Store {
    /// Register (or revise) an artifact keyed by `(card_id, path)` (`dflow plan open`).
    ///
    /// A first registration inserts `{ doc_id, round: 1, status: open }`. Re-registering
    /// the same path is a revision: the round bumps, a fresh `revised_doc_id` is minted
    /// (so the iframe reloads), the `doc_id` stays stable (its served file is overwritten
    /// in place by the daemon), and the status returns to `open`. Returns
    /// `(row, revised)`.
    pub fn register_artifact(
        &self,
        card_id: &str,
        path: &str,
        kind: &str,
        title: Option<&str>,
    ) -> Result<(ArtifactRow, bool), StoreError> {
        let ts = now_ms();
        if let Some(existing) = self.get_artifact_by_path(card_id, path)? {
            let revised_doc_id = new_ulid().to_string();
            let round = existing.round + 1;
            {
                let conn = self.lock();
                conn.execute(
                    "UPDATE artifacts SET kind = ?2, title = COALESCE(?3, title), \
                     revised_doc_id = ?4, round = ?5, status = ?6, updated_at = ?7 WHERE id = ?1",
                    params![
                        existing.id,
                        kind,
                        title,
                        revised_doc_id,
                        round,
                        artifact_status::OPEN,
                        ts
                    ],
                )?;
            }
            let row = self
                .get_artifact(&existing.id)?
                .ok_or_else(|| StoreError::NotFound(format!("artifact {}", existing.id)))?;
            return Ok((row, true));
        }
        let id = new_ulid().to_string();
        let doc_id = new_ulid().to_string();
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO artifacts \
                 (id, card_id, path, kind, title, doc_id, revised_doc_id, round, audit, status, \
                  created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 1, NULL, ?7, ?8, ?8)",
                params![id, card_id, path, kind, title, doc_id, artifact_status::OPEN, ts],
            )?;
        }
        let row = self
            .get_artifact(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("artifact {id} vanished after insert")))?;
        Ok((row, false))
    }

    /// Fetch an artifact by id.
    pub fn get_artifact(&self, id: &str) -> Result<Option<ArtifactRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{ARTIFACT_SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], artifact_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Fetch an artifact by its serving `doc_id` (the HTTP endpoint's identity).
    pub fn get_artifact_by_doc(&self, doc_id: &str) -> Result<Option<ArtifactRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{ARTIFACT_SELECT} WHERE doc_id = ?1"))?;
        let mut rows = stmt.query_map(params![doc_id], artifact_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Fetch an artifact by `(card_id, path)`.
    pub fn get_artifact_by_path(
        &self,
        card_id: &str,
        path: &str,
    ) -> Result<Option<ArtifactRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{ARTIFACT_SELECT} WHERE card_id = ?1 AND path = ?2"))?;
        let mut rows = stmt.query_map(params![card_id, path], artifact_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Every artifact for a card, newest first.
    pub fn card_artifacts(&self, card_id: &str) -> Result<Vec<ArtifactRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{ARTIFACT_SELECT} WHERE card_id = ?1 ORDER BY id DESC"))?;
        let rows = stmt.query_map(params![card_id], artifact_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The card's active plan artifact: the newest `plan`-kind artifact that is not yet
    /// ended, so `dflow plan poll` (which carries only the card) can resolve it.
    pub fn active_plan_artifact(&self, card_id: &str) -> Result<Option<ArtifactRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "{ARTIFACT_SELECT} WHERE card_id = ?1 AND kind = 'plan' \
             AND status NOT IN ('approved','ended') ORDER BY id DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query_map(params![card_id], artifact_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => {
                // No open plan: fall back to the newest plan of any status so a poll after
                // approval still resolves the artifact and reports `ended`.
                drop(rows);
                let mut stmt = conn.prepare(&format!(
                    "{ARTIFACT_SELECT} WHERE card_id = ?1 AND kind = 'plan' ORDER BY id DESC LIMIT 1"
                ))?;
                let mut rows = stmt.query_map(params![card_id], artifact_from_row)?;
                match rows.next() {
                    Some(row) => Ok(Some(row?)),
                    None => Ok(None),
                }
            }
        }
    }

    /// Set an artifact's status.
    pub fn set_artifact_status(&self, id: &str, status: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE artifacts SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status, now_ms()],
        )?;
        Ok(())
    }

    /// Store the latest layout-audit result (`layout_warnings` json) on an artifact.
    pub fn set_artifact_audit(&self, id: &str, audit_json: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE artifacts SET audit = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, audit_json, now_ms()],
        )?;
        Ok(())
    }

    /// Append a batch of feedback items as annotations for `round`, state `queued`
    /// (undelivered), so a poll can consume them later. The full item JSON is stored so
    /// the poll replays the exact wire item.
    pub fn add_annotations(
        &self,
        artifact_id: &str,
        round: i64,
        items: &[FeedbackItem],
    ) -> Result<usize, StoreError> {
        let conn = self.lock();
        for item in items {
            let anchor = item
                .anchor
                .as_ref()
                .and_then(|a| serde_json::to_string(a).ok());
            let payload = serde_json::to_string(item)?;
            conn.execute(
                "INSERT INTO annotations (id, artifact_id, round, kind, anchor, body, payload, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued')",
                params![
                    new_ulid().to_string(),
                    artifact_id,
                    round,
                    item.kind,
                    anchor,
                    item.body,
                    payload,
                ],
            )?;
        }
        Ok(items.len())
    }

    /// Whether an artifact has any un-delivered (`queued`) annotations.
    pub fn has_queued_annotations(&self, artifact_id: &str) -> Result<bool, StoreError> {
        let conn = self.lock();
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM annotations WHERE artifact_id = ?1 AND state = 'queued'",
            params![artifact_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Consume the lowest-round queued batch: mark its annotations `sent` and return
    /// `(round, items)`. Returns `None` when nothing is queued (the poll then parks or
    /// reports pending). One transaction so a batch is delivered exactly once.
    pub fn take_queued_batch(
        &self,
        artifact_id: &str,
    ) -> Result<Option<(i64, Vec<FeedbackItem>)>, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let round: Option<i64> = tx
            .query_row(
                "SELECT min(round) FROM annotations WHERE artifact_id = ?1 AND state = 'queued'",
                params![artifact_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        let round = match round {
            Some(r) => r,
            None => return Ok(None),
        };
        let items: Vec<FeedbackItem> = {
            let mut stmt = tx.prepare(
                "SELECT payload FROM annotations \
                 WHERE artifact_id = ?1 AND round = ?2 AND state = 'queued' ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![artifact_id, round], |r| {
                let payload: String = r.get(0)?;
                Ok(payload)
            })?;
            rows.filter_map(|r| r.ok())
                .filter_map(|p| serde_json::from_str::<FeedbackItem>(&p).ok())
                .collect()
        };
        tx.execute(
            "UPDATE annotations SET state = 'sent' \
             WHERE artifact_id = ?1 AND round = ?2 AND state = 'queued'",
            params![artifact_id, round],
        )?;
        tx.commit()?;
        Ok(Some((round, items)))
    }
}

const ARTIFACT_SELECT: &str = "SELECT id, card_id, path, kind, title, doc_id, revised_doc_id, \
     round, audit, status, created_at, updated_at FROM artifacts";

fn artifact_from_row(row: &Row) -> rusqlite::Result<ArtifactRow> {
    Ok(ArtifactRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        path: row.get(2)?,
        kind: row.get(3)?,
        title: row.get(4)?,
        doc_id: row.get(5)?,
        revised_doc_id: row.get(6)?,
        round: row.get(7)?,
        audit: row.get(8)?,
        status: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewCard, Store};
    use dflow_proto::{FeedbackItem, TextAnchor};

    fn store_with_card() -> (Store, String) {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project("/tmp/art", "art", "main", "pr").unwrap();
        let card = store
            .create_card(NewCard { project_id: Some(project.id), title: "Plan me".into(), ..Default::default() })
            .unwrap();
        (store, card.id)
    }

    fn text_item(quote: &str, body: &str, status: &str) -> FeedbackItem {
        FeedbackItem {
            kind: "text_range".into(),
            anchor: Some(TextAnchor { selector: "#retry".into(), start: 0, end: quote.len() as i64, quote: quote.into() }),
            body: Some(body.into()),
            question_key: None,
            value: None,
            diagram: None,
            node: None,
            action: None,
            status: Some(status.into()),
        }
    }

    #[test]
    fn register_then_revision_keeps_doc_id_and_bumps_round() {
        let (store, card_id) = store_with_card();
        let (first, revised) = store.register_artifact(&card_id, "plan.html", "plan", Some("Plan")).unwrap();
        assert!(!revised);
        assert_eq!(first.round, 1);
        assert_eq!(first.status, artifact_status::OPEN);
        assert!(first.revised_doc_id.is_none());

        let (second, revised) = store.register_artifact(&card_id, "plan.html", "plan", None).unwrap();
        assert!(revised, "re-registering the same path is a revision");
        assert_eq!(second.id, first.id, "same artifact row");
        assert_eq!(second.doc_id, first.doc_id, "doc_id stays stable (served in place)");
        assert_eq!(second.round, 2, "round bumps on revision");
        assert!(second.revised_doc_id.is_some(), "a revision nonce is minted for the iframe reload");
        assert_ne!(second.revised_doc_id, first.revised_doc_id);
        // Title is preserved when the revision omits it.
        assert_eq!(second.title.as_deref(), Some("Plan"));
    }

    #[test]
    fn annotation_batches_deliver_once_in_round_order() {
        let (store, card_id) = store_with_card();
        let (art, _) = store.register_artifact(&card_id, "plan.html", "plan", None).unwrap();

        // Round 1 feedback, then round 2.
        store.add_annotations(&art.id, 1, &[text_item("retry", "cap at 3", "anchored")]).unwrap();
        store.add_annotations(&art.id, 2, &[text_item("dead-letter", "alert on it", "drifted")]).unwrap();
        assert!(store.has_queued_annotations(&art.id).unwrap());

        // The lowest round is delivered first, exactly once.
        let (round, items) = store.take_queued_batch(&art.id).unwrap().unwrap();
        assert_eq!(round, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].body.as_deref(), Some("cap at 3"));
        assert_eq!(items[0].status.as_deref(), Some("anchored"));

        let (round, items) = store.take_queued_batch(&art.id).unwrap().unwrap();
        assert_eq!(round, 2);
        assert_eq!(items[0].status.as_deref(), Some("drifted"));

        // Nothing left; delivery is idempotent (no double-send).
        assert!(store.take_queued_batch(&art.id).unwrap().is_none());
        assert!(!store.has_queued_annotations(&art.id).unwrap());
    }

    #[test]
    fn active_plan_artifact_resolves_the_open_plan() {
        let (store, card_id) = store_with_card();
        assert!(store.active_plan_artifact(&card_id).unwrap().is_none());
        let (art, _) = store.register_artifact(&card_id, "plan.html", "plan", None).unwrap();
        let active = store.active_plan_artifact(&card_id).unwrap().unwrap();
        assert_eq!(active.id, art.id);
        // Once approved it is no longer the "open" plan, but the newest plan still resolves.
        store.set_artifact_status(&art.id, artifact_status::APPROVED).unwrap();
        let resolved = store.active_plan_artifact(&card_id).unwrap().unwrap();
        assert_eq!(resolved.id, art.id);
        assert_eq!(resolved.status, artifact_status::APPROVED);
    }

    #[test]
    fn audit_round_trips_as_layout_warnings() {
        let (store, card_id) = store_with_card();
        let (art, _) = store.register_artifact(&card_id, "plan.html", "plan", None).unwrap();
        let warnings = serde_json::json!([
            { "selector": "html", "kind": "horizontal_overflow", "overflow_px": 674, "viewport_width": 1200, "severity": "error" }
        ]);
        store.set_artifact_audit(&art.id, &warnings.to_string()).unwrap();
        let reloaded = store.get_artifact(&art.id).unwrap().unwrap();
        let lw = reloaded.layout_warnings();
        assert_eq!(lw.len(), 1);
        assert_eq!(lw[0].kind, "horizontal_overflow");
        assert_eq!(lw[0].severity, "error");
    }
}
