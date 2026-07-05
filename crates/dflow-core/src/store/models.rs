//! Engine-internal row structs for tables whose full shape is not on the wire.
//!
//! Wire-facing entities (`Project`, `Card`, `CardEvent`, `SessionSummary`) are the
//! `dflow_proto` types, constructed directly by the store. The structs here carry
//! columns the daemon needs but does not necessarily forward verbatim (worktree
//! paths, scrollback paths, resume lineage, needs-you bookkeeping).

use serde::{Deserialize, Serialize};

/// A row of the `sessions` table (`data-model.md` / sessions), with the Phase 1.5
/// `title`/`agent_id` columns included.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    /// Nullable since session-first (`data-model.md`): cardless sessions are legitimate.
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
    pub resume_ref: Option<String>,
    pub resumed_from: Option<String>,
    pub first_prompt: Option<String>,
    /// User-renamable tab label (Phase 1.5 addendum); null means generated.
    pub title: Option<String>,
    /// The agent's last tier-1 status note (`dflow status`/`dflow card note`), read by
    /// the board session strip (M2 tier-1 signals). Null until self-reported.
    pub status_note: Option<String>,
    /// Reference into the `agents` table (Phase 1.5 addendum); unused by dispatch now.
    pub agent_id: Option<String>,
    /// Epoch milliseconds.
    pub created_at: i64,
    pub ended_at: Option<i64>,
}

/// A row of the `worktrees` table (`data-model.md` / worktrees).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeRow {
    pub id: String,
    pub project_id: String,
    pub slot: i64,
    pub path: String,
    /// `available | leased | dirty | retired`.
    pub lease_state: String,
    pub leased_by_card: Option<String>,
    pub cache_meta: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A row of the `gate_runs` table (`data-model.md` / gate_runs, `gate.md` / Pipeline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRunRow {
    pub id: String,
    pub card_id: String,
    pub worktree_id: Option<String>,
    /// `checks | review | autofix | escalate | push | pr | ci | done`.
    pub step: String,
    /// `running | passed | failed | escalated`.
    pub status: String,
    /// `full | checks_only | none` (the recipe's declared strictness).
    pub gate_strictness: Option<String>,
    pub author_harness: Option<String>,
    pub reviewer_harness: Option<String>,
    pub head_sha: Option<String>,
    pub branch: Option<String>,
    pub pr_number: Option<i64>,
    pub pr_url: Option<String>,
    pub output_path: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// A row of the `findings` table (`data-model.md` / findings, `gate.md` / Findings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingRow {
    pub id: String,
    pub gate_run_id: String,
    pub card_id: String,
    /// `blocker | major | minor`.
    pub severity: String,
    /// `mechanical | intent` (autofix routing).
    pub category: String,
    /// `reviewer | check | ci`.
    pub source: String,
    pub body: String,
    pub evidence: Option<String>,
    /// `autofixed | accepted | fixed | skipped`; `None` while open.
    pub resolution: Option<String>,
    pub created_at: Option<i64>,
    pub resolved_at: Option<i64>,
}

impl FindingRow {
    /// Whether this finding is still open (unresolved).
    pub fn is_open(&self) -> bool {
        self.resolution.is_none()
    }
}

/// A row of the `needs_you_items` table (`data-model.md` / needs_you_items).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsYouItem {
    pub id: String,
    pub card_id: String,
    pub kind: String,
    pub dedupe_key: String,
    pub score: i64,
    pub raised_at: i64,
    pub notified_at: Option<i64>,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
}
