//! Gate-run and finding store methods (`data-model.md` / gate_runs, findings;
//! `gate.md` / Pipeline, Findings).
//!
//! A gate run is one pass of checks -> adversarial review -> autofix -> escalation ->
//! ship for a card. Every lifecycle change appends a `card_events` row inside the same
//! transaction and broadcasts it after commit (`tx_events`), so the timeline shows
//! exactly why a branch was (or was not) allowed out. Findings and their resolutions are
//! events with evidence pointers, never prose-only claims.

use rusqlite::{params, Row};

use super::{append_event, event_kind, now_ms, FindingRow, GateRunRow, Store, StoreError};

/// Parameters for opening a gate run.
#[derive(Debug, Clone, Default)]
pub struct NewGateRun {
    pub card_id: String,
    pub worktree_id: Option<String>,
    pub gate_strictness: Option<String>,
    pub author_harness: Option<String>,
    pub reviewer_harness: Option<String>,
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub output_path: Option<String>,
}

/// `gate_runs.step` values (`data-model.md` / gate_runs.step).
pub mod gate_step {
    pub const CHECKS: &str = "checks";
    pub const REVIEW: &str = "review";
    pub const AUTOFIX: &str = "autofix";
    pub const ESCALATE: &str = "escalate";
    pub const PUSH: &str = "push";
    pub const PR: &str = "pr";
    pub const CI: &str = "ci";
    pub const DONE: &str = "done";
}

/// `gate_runs.status` values.
pub mod gate_status {
    pub const RUNNING: &str = "running";
    pub const PASSED: &str = "passed";
    pub const FAILED: &str = "failed";
    pub const ESCALATED: &str = "escalated";
}

/// `findings.severity` values.
pub mod severity {
    pub const BLOCKER: &str = "blocker";
    pub const MAJOR: &str = "major";
    pub const MINOR: &str = "minor";

    /// Whether a token is a valid severity.
    pub fn is_valid(s: &str) -> bool {
        matches!(s, BLOCKER | MAJOR | MINOR)
    }
}

/// `findings.category` values (autofix routing, `gate.md` / Autofix, Escalation).
pub mod category {
    /// Safe-mechanical: lint, formatting, dead imports, trivial test fixes -> autofix.
    pub const MECHANICAL: &str = "mechanical";
    /// Intent-touching: behavior, API shape, scope -> escalation as a Needs You item.
    pub const INTENT: &str = "intent";

    pub fn is_valid(s: &str) -> bool {
        matches!(s, MECHANICAL | INTENT)
    }
}

/// `findings.resolution` values.
pub mod resolution {
    pub const AUTOFIXED: &str = "autofixed";
    pub const ACCEPTED: &str = "accepted";
    pub const FIXED: &str = "fixed";
    pub const SKIPPED: &str = "skipped";
}

impl Store {
    /// Open a gate run for a card and append `gate_started`.
    pub fn create_gate_run(&self, new: NewGateRun) -> Result<GateRunRow, StoreError> {
        let id = super::new_ulid().to_string();
        let ts = now_ms();
        self.tx_events(|tx, events| {
            tx.execute(
                "INSERT INTO gate_runs \
                 (id, card_id, worktree_id, step, status, gate_strictness, author_harness, \
                  reviewer_harness, head_sha, branch, output_path, started_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    id,
                    new.card_id,
                    new.worktree_id,
                    gate_step::CHECKS,
                    gate_status::RUNNING,
                    new.gate_strictness,
                    new.author_harness,
                    new.reviewer_harness,
                    new.head_sha,
                    new.branch,
                    new.output_path,
                    ts,
                ],
            )?;
            append_event(
                tx,
                events,
                &new.card_id,
                event_kind::GATE_STARTED,
                serde_json::json!({
                    "gate_run_id": id,
                    "strictness": new.gate_strictness,
                    "worktree_id": new.worktree_id,
                    "head_sha": new.head_sha,
                    "reviewer_harness": new.reviewer_harness,
                }),
            )?;
            Ok(())
        })?;
        self.get_gate_run(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("gate_run {id}")))
    }

    /// Advance the run's current step (a bookkeeping update; the `gate_step` event with
    /// evidence is recorded separately by [`Store::record_gate_step`]).
    pub fn set_gate_run_step(&self, id: &str, step: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute("UPDATE gate_runs SET step = ?2 WHERE id = ?1", params![id, step])?;
        Ok(())
    }

    /// Record a `gate_step` event carrying an evidence pointer (exit codes, log paths).
    pub fn record_gate_step(
        &self,
        card_id: &str,
        gate_run_id: &str,
        step: &str,
        status: &str,
        evidence: serde_json::Value,
    ) -> Result<(), StoreError> {
        self.set_gate_run_step(gate_run_id, step)?;
        self.append_card_event(
            card_id,
            event_kind::GATE_STEP,
            serde_json::json!({
                "gate_run_id": gate_run_id,
                "step": step,
                "status": status,
                "evidence": evidence,
            }),
        )?;
        Ok(())
    }

    /// Bind the leased gate-class worktree to the run.
    pub fn set_gate_run_worktree(&self, id: &str, worktree_id: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute("UPDATE gate_runs SET worktree_id = ?2 WHERE id = ?1", params![id, worktree_id])?;
        Ok(())
    }

    /// Record the gate evidence directory on the run.
    pub fn set_gate_run_output_path(&self, id: &str, path: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute("UPDATE gate_runs SET output_path = ?2 WHERE id = ?1", params![id, path])?;
        Ok(())
    }

    /// Update the commit under test (e.g. after autofix advances HEAD).
    pub fn set_gate_run_head(&self, id: &str, head_sha: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute("UPDATE gate_runs SET head_sha = ?2 WHERE id = ?1", params![id, head_sha])?;
        Ok(())
    }

    /// Record the PR a ship opened on the gate run.
    pub fn set_gate_run_pr(&self, id: &str, pr_number: i64, pr_url: &str) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.execute(
            "UPDATE gate_runs SET pr_number = ?2, pr_url = ?3 WHERE id = ?1",
            params![id, pr_number, pr_url],
        )?;
        Ok(())
    }

    /// Finish a gate run with a terminal status, appending `gate_passed` or `gate_failed`.
    pub fn finish_gate_run(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
    ) -> Result<(), StoreError> {
        let run = self
            .get_gate_run(id)?
            .ok_or_else(|| StoreError::NotFound(format!("gate_run {id}")))?;
        let ts = now_ms();
        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE gate_runs SET status = ?2, step = ?3, ended_at = ?4 WHERE id = ?1",
                params![id, status, gate_step::DONE, ts],
            )?;
            let kind = if status == gate_status::PASSED {
                event_kind::GATE_PASSED
            } else {
                event_kind::GATE_FAILED
            };
            append_event(
                tx,
                events,
                &run.card_id,
                kind,
                serde_json::json!({ "gate_run_id": id, "status": status, "reason": reason }),
            )?;
            Ok(())
        })
    }

    /// Fetch a gate run by id.
    pub fn get_gate_run(&self, id: &str) -> Result<Option<GateRunRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{GATE_SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], gate_run_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// The most recent gate run for a card, or `None`.
    pub fn latest_gate_run_for_card(&self, card_id: &str) -> Result<Option<GateRunRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{GATE_SELECT} WHERE card_id = ?1 ORDER BY id DESC LIMIT 1"))?;
        let mut rows = stmt.query_map(params![card_id], gate_run_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// File a finding against a gate run and append `finding_raised`.
    #[allow(clippy::too_many_arguments)]
    pub fn add_finding(
        &self,
        gate_run_id: &str,
        card_id: &str,
        severity: &str,
        cat: &str,
        source: &str,
        body: &str,
        evidence: Option<&str>,
    ) -> Result<FindingRow, StoreError> {
        if !severity::is_valid(severity) {
            return Err(StoreError::Invalid(format!(
                "severity must be blocker|major|minor, got '{severity}'"
            )));
        }
        if !category::is_valid(cat) {
            return Err(StoreError::Invalid(format!(
                "category must be mechanical|intent, got '{cat}'"
            )));
        }
        let id = super::new_ulid().to_string();
        let ts = now_ms();
        self.tx_events(|tx, events| {
            tx.execute(
                "INSERT INTO findings \
                 (id, gate_run_id, card_id, severity, category, source, body, evidence, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![id, gate_run_id, card_id, severity, cat, source, body, evidence, ts],
            )?;
            append_event(
                tx,
                events,
                card_id,
                event_kind::FINDING_RAISED,
                serde_json::json!({
                    "finding_id": id,
                    "gate_run_id": gate_run_id,
                    "severity": severity,
                    "category": cat,
                    "source": source,
                }),
            )?;
            Ok(())
        })?;
        self.get_finding(&id)?
            .ok_or_else(|| StoreError::NotFound(format!("finding {id}")))
    }

    /// Resolve a finding and append `finding_resolved`. Idempotent: an already-resolved
    /// finding is left unchanged and returned as-is.
    pub fn resolve_finding(
        &self,
        finding_id: &str,
        res: &str,
    ) -> Result<Option<FindingRow>, StoreError> {
        let existing = match self.get_finding(finding_id)? {
            Some(f) => f,
            None => return Ok(None),
        };
        if existing.resolution.is_some() {
            return Ok(Some(existing));
        }
        let ts = now_ms();
        self.tx_events(|tx, events| {
            tx.execute(
                "UPDATE findings SET resolution = ?2, resolved_at = ?3 WHERE id = ?1",
                params![finding_id, res, ts],
            )?;
            append_event(
                tx,
                events,
                &existing.card_id,
                event_kind::FINDING_RESOLVED,
                serde_json::json!({ "finding_id": finding_id, "resolution": res }),
            )?;
            Ok(())
        })?;
        self.get_finding(finding_id)
    }

    /// Fetch a finding by id.
    pub fn get_finding(&self, id: &str) -> Result<Option<FindingRow>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{FINDING_SELECT} WHERE id = ?1"))?;
        let mut rows = stmt.query_map(params![id], finding_from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Every finding filed against a gate run, oldest first.
    pub fn findings_for_run(&self, gate_run_id: &str) -> Result<Vec<FindingRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{FINDING_SELECT} WHERE gate_run_id = ?1 ORDER BY id ASC"))?;
        let rows = stmt.query_map(params![gate_run_id], finding_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Every finding filed against a card across its gate runs, newest first.
    pub fn findings_for_card(&self, card_id: &str) -> Result<Vec<FindingRow>, StoreError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare(&format!("{FINDING_SELECT} WHERE card_id = ?1 ORDER BY id DESC"))?;
        let rows = stmt.query_map(params![card_id], finding_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

const GATE_SELECT: &str = "SELECT id, card_id, worktree_id, step, status, gate_strictness, \
     author_harness, reviewer_harness, head_sha, branch, pr_number, pr_url, output_path, \
     started_at, ended_at FROM gate_runs";

fn gate_run_from_row(row: &Row) -> rusqlite::Result<GateRunRow> {
    Ok(GateRunRow {
        id: row.get(0)?,
        card_id: row.get(1)?,
        worktree_id: row.get(2)?,
        step: row.get(3)?,
        status: row.get(4)?,
        gate_strictness: row.get(5)?,
        author_harness: row.get(6)?,
        reviewer_harness: row.get(7)?,
        head_sha: row.get(8)?,
        branch: row.get(9)?,
        pr_number: row.get(10)?,
        pr_url: row.get(11)?,
        output_path: row.get(12)?,
        started_at: row.get(13)?,
        ended_at: row.get(14)?,
    })
}

const FINDING_SELECT: &str = "SELECT id, gate_run_id, card_id, severity, category, source, body, \
     evidence, resolution, created_at, resolved_at FROM findings";

fn finding_from_row(row: &Row) -> rusqlite::Result<FindingRow> {
    Ok(FindingRow {
        id: row.get(0)?,
        gate_run_id: row.get(1)?,
        card_id: row.get(2)?,
        severity: row.get(3)?,
        category: row.get(4)?,
        source: row.get(5)?,
        body: row.get(6)?,
        evidence: row.get(7)?,
        resolution: row.get(8)?,
        created_at: row.get(9)?,
        resolved_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn setup() -> (Store, String) {
        let store = Store::open_in_memory().unwrap();
        let project = store.add_project("/tmp/gate", "gate", "main", "pr").unwrap();
        let card = store
            .create_card(crate::store::NewCard {
                project_id: Some(project.id),
                title: "gate me".into(),
                ..Default::default()
            })
            .unwrap();
        (store, card.id)
    }

    #[test]
    fn gate_run_lifecycle_and_events() {
        let (store, card_id) = setup();
        let run = store
            .create_gate_run(NewGateRun {
                card_id: card_id.clone(),
                gate_strictness: Some("full".into()),
                head_sha: Some("abc123".into()),
                reviewer_harness: Some("codex".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(run.status, gate_status::RUNNING);
        assert_eq!(run.step, gate_step::CHECKS);

        store
            .record_gate_step(&card_id, &run.id, gate_step::CHECKS, "passed", serde_json::json!({ "exit": 0 }))
            .unwrap();
        store.finish_gate_run(&run.id, gate_status::PASSED, None).unwrap();

        let after = store.get_gate_run(&run.id).unwrap().unwrap();
        assert_eq!(after.status, gate_status::PASSED);
        assert!(after.ended_at.is_some());

        // The timeline carries gate_started -> gate_step -> gate_passed.
        let events = store.card_events(&card_id, None, 100).unwrap();
        let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
        assert!(kinds.contains(&event_kind::GATE_STARTED));
        assert!(kinds.contains(&event_kind::GATE_STEP));
        assert!(kinds.contains(&event_kind::GATE_PASSED));
        assert_eq!(store.latest_gate_run_for_card(&card_id).unwrap().unwrap().id, run.id);
    }

    #[test]
    fn findings_raise_resolve_and_query() {
        let (store, card_id) = setup();
        let run = store
            .create_gate_run(NewGateRun { card_id: card_id.clone(), ..Default::default() })
            .unwrap();

        let f1 = store
            .add_finding(&run.id, &card_id, severity::MINOR, category::MECHANICAL, "reviewer", "unused import", None)
            .unwrap();
        let f2 = store
            .add_finding(&run.id, &card_id, severity::BLOCKER, category::INTENT, "reviewer", "wrong API shape", None)
            .unwrap();
        assert!(f1.is_open() && f2.is_open());
        assert_eq!(store.findings_for_run(&run.id).unwrap().len(), 2);

        let resolved = store.resolve_finding(&f1.id, resolution::AUTOFIXED).unwrap().unwrap();
        assert_eq!(resolved.resolution.as_deref(), Some(resolution::AUTOFIXED));
        assert!(!resolved.is_open());

        // Idempotent resolve leaves the first resolution.
        let again = store.resolve_finding(&f1.id, resolution::SKIPPED).unwrap().unwrap();
        assert_eq!(again.resolution.as_deref(), Some(resolution::AUTOFIXED));

        // f2 stays open.
        let open: Vec<_> = store.findings_for_run(&run.id).unwrap().into_iter().filter(|f| f.is_open()).collect();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, f2.id);

        // A bad severity is rejected.
        assert!(store
            .add_finding(&run.id, &card_id, "critical", category::INTENT, "reviewer", "x", None)
            .is_err());
    }
}
