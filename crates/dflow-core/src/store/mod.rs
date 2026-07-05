//! The SQLite store: the daemon's source of truth (`data-model.md`).
//!
//! # Concurrency choice
//!
//! `data-model.md` allows either a dedicated writer task or a mutex-guarded
//! connection "at this scale". This store uses a single `Mutex<Connection>`: every
//! read and write serializes through one connection. The operations here are tiny,
//! single-row local SQLite statements on a WAL database, so the lock is held for
//! microseconds; the daemon calls these synchronous methods from its async handlers
//! without holding the guard across an `.await`. If write contention ever shows up
//! in a profile, this is the one place to swap in a writer task behind the same API.
//!
//! # Migrations
//!
//! Schema versions are tracked with SQLite's `user_version` pragma. Migrations are
//! forward-only SQL scripts applied at open, each in its own transaction. Opening a
//! database whose `user_version` is newer than this build knows is refused
//! (`StoreError::SchemaTooNew`) rather than risking a corrupt downgrade.
//!
//! # Events
//!
//! Every card-scoped mutation appends a `card_events` row inside the same
//! transaction as the state change, then broadcasts the committed event to
//! subscribers (`event.subscribe`). Broadcast happens only after commit, so a
//! subscriber never sees an event that later rolls back.

mod agents;
mod artifacts;
mod cards;
pub mod env;
mod gate;
mod knowledge;
mod models;
mod needs_you;
mod phone;
mod projects;
mod recipes;
mod services;
mod settings;
mod sessions;
mod worktrees;

pub use agents::{agent_source, AgentPatch, DetectionOutcome, NewAgent};
pub use artifacts::{artifact_status, ArtifactRow};
pub use cards::{CardPatch, CardQueryFilter, NewCard, OriginUpsert};
pub use env::{env_kind, EnvEntryMeta};
pub use gate::{category, gate_status, gate_step, resolution, severity, NewGateRun};
pub use knowledge::KnowledgeRow;
pub use models::{FindingRow, GateRunRow, NeedsYouItem, SessionRow, WorktreeRow};
pub use phone::PhoneTokenRow;
pub use recipes::{RecipeGrant, RecipeIndexRow};
pub use services::{service_scope, ServiceRow};
pub use settings::setting_key;
pub use sessions::NewSession;

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, Row, Transaction};
use tokio::sync::broadcast;
use ulid::{Generator, Ulid};

use dflow_proto::{CardEvent, CheckCmd, Project};

/// Broadcast backlog of committed card events for live `event.subscribe` streams.
/// A subscriber that lags beyond this recovers by re-reading from its cursor.
const EVENT_BROADCAST_CAPACITY: usize = 4096;

/// `card_events.kind` values (`data-model.md` / Event taxonomy).
pub mod event_kind {
    pub const CREATED: &str = "created";
    pub const SHAPED: &str = "shaped";
    pub const MOVED: &str = "moved";
    pub const DIAL_CHANGED: &str = "dial_changed";
    pub const CLOSED: &str = "closed";
    pub const DISPATCHED: &str = "dispatched";
    pub const WORKTREE_LEASED: &str = "worktree_leased";
    /// The env vault was materialized into the session's worktree at dispatch
    /// (`data-model.md` / Dispatch; `environments.md`). Payload records counts and file
    /// target names, never any value.
    pub const ENV_MATERIALIZED: &str = "env_materialized";
    /// The dispatch brief was composed (`adapters.md` dispatch flow step 6); the
    /// payload records which recipe guidance sections were injected, so timelines
    /// carry evidence of what the agent was told, not just that it was launched.
    pub const BRIEF_COMPOSED: &str = "brief_composed";
    pub const SESSION_STARTED: &str = "session_started";
    pub const STATE_CHANGED: &str = "state_changed";
    pub const TURN_ENDED: &str = "turn_ended";
    pub const NEEDS_INPUT: &str = "needs_input";
    /// An agent self-reported `blocked` via `dflow status blocked <note>` (tier-1).
    pub const BLOCKED: &str = "blocked";
    pub const SESSION_ENDED: &str = "session_ended";
    /// A resume created a new session linked to a predecessor (`resumed_from`); the UI
    /// renders a "session resumed" divider from this event (`architecture.md`).
    pub const SESSION_RESUMED: &str = "session_resumed";
    pub const WORKTREE_RETURNED: &str = "worktree_returned";
    pub const NEEDS_YOU_RAISED: &str = "needs_you_raised";
    pub const NEEDS_YOU_RESOLVED: &str = "needs_you_resolved";
    /// An agent recorded a knowledge note via `dflow know add` (`knowledge.md` /
    /// Data-model touchpoints; payload: note path, type, title, verb).
    pub const KNOWLEDGE_UPDATED: &str = "knowledge_updated";
    /// A Concertmaster (mcp-scoped client) steered a session via
    /// `session.send_verified` (`data-model.md` / Concertmaster M4; payload: injected
    /// text, session id). Emitted only for an mcp-scoped connection.
    pub const CONCERTMASTER_STEERED: &str = "concertmaster_steered";
    /// A Concertmaster round was dispatched (`product.md` / Concertmaster rounds; M4).
    /// Payload: round type, scope, session id. The round card anchors the timeline.
    pub const ROUND_STARTED: &str = "round_started";
    /// A Concertmaster round filed its digest (`data-model.md` / Concertmaster M4:
    /// `round_completed`; payload: round type, scope, findings count, digest item id).
    /// The escalation-only output contract: at most one deduplicated Needs You digest.
    pub const ROUND_COMPLETED: &str = "round_completed";

    // ---- Plan Studio (`data-model.md` / Event taxonomy; `plan-studio.md`) ----
    /// An agent registered a plan artifact for review (`dflow plan open`). Payload:
    /// artifact id, kind, doc_id, round.
    pub const ARTIFACT_OPENED: &str = "artifact_opened";
    /// A review round opened (a revision was posted for review). Payload: artifact id,
    /// round, revised_doc_id.
    pub const PLAN_ROUND: &str = "plan_round";
    /// The human sent a feedback batch (`artifact.feedback.submit`). Payload: artifact
    /// id, round, item count (never the note bodies verbatim beyond the scrubber).
    pub const FEEDBACK_SENT: &str = "feedback_sent";
    /// The human approved the plan (a first-class Approve action). Payload: artifact id,
    /// round.
    pub const PLAN_APPROVED: &str = "plan_approved";
    /// The plan review ended (approved or ended by the human). Payload: artifact id,
    /// reason, round.
    pub const ARTIFACT_ENDED: &str = "artifact_ended";

    // ---- Environments (M3 services + drift guard, `environments.md`) ----
    /// A declared per-worktree service started at dispatch, with its allocated ports
    /// (`environments.md` / Local services and the port broker). Payload: service name,
    /// allocated `DFLOW_PORT_<NAME>` map, pid.
    pub const SERVICE_STARTED: &str = "service_started";
    /// A required service failed its process-alive health check at dispatch, parking the
    /// card in Needs You (`environments.md`). Payload: service name, reason.
    pub const SERVICE_FAILED: &str = "service_failed";
    /// A materialized env file drifted from the vault at worktree return; the payload is
    /// a value-masked diff summary (`environments.md` / Drift guard). The raise side of
    /// the loop; absorbing is a later `env.set`. Payload: target, added/removed/changed
    /// keys (never values).
    pub const ENV_DRIFT: &str = "env_drift";

    // ---- Gate (`gate.md` / Pipeline; `data-model.md` / Event taxonomy: Gate) ----
    /// A gate run started for a card (`dflow` verify or the user's Verify click). Payload:
    /// gate_run_id, strictness, worktree_id, head_sha.
    pub const GATE_STARTED: &str = "gate_started";
    /// A gate pipeline step ran; the payload carries the step name, its status, and an
    /// evidence pointer (exit codes, log paths), never a prose-only claim.
    pub const GATE_STEP: &str = "gate_step";
    /// An adversarial-review (or check/ci) finding was raised via `dflow finding add`.
    /// Payload: finding_id, severity, category, source (never the raw finding body beyond
    /// the scrubber).
    pub const FINDING_RAISED: &str = "finding_raised";
    /// A finding was resolved (autofixed, accepted, fixed, or skipped). Payload:
    /// finding_id, resolution.
    pub const FINDING_RESOLVED: &str = "finding_resolved";
    /// A gate run passed: every check green and every finding resolved (`gate.md`).
    /// Payload: gate_run_id.
    pub const GATE_PASSED: &str = "gate_passed";
    /// A gate run failed or escalated (a check went red, or an intent-touching finding
    /// became a Needs You). Payload: gate_run_id, reason.
    pub const GATE_FAILED: &str = "gate_failed";

    // ---- Delivery (`data-model.md` / Event taxonomy: Delivery; `gate.md` / Ship) ----
    /// The gate/ship branch was pushed through the git credential helper. Payload:
    /// branch, head_sha, remote.
    pub const PUSHED: &str = "pushed";
    /// A pull request was opened via gh with the generated summary. Payload: pr_number,
    /// pr_url, base, head, fixes (the origin issue number, when the card is an issue).
    pub const PR_OPENED: &str = "pr_opened";
    /// A CI status snapshot streamed back from `gh pr checks`. Payload: pr_number, passing,
    /// pending, failing, checks.
    pub const CI_STATUS: &str = "ci_status";
    /// A PR was merged (squash default). Payload: pr_number, method.
    pub const MERGED: &str = "merged";
}

/// `sessions.state` values (`adapters.md` / lifecycle states, plus `interrupted`).
pub mod session_state {
    pub const STARTING: &str = "starting";
    pub const WORKING: &str = "working";
    pub const IDLE: &str = "idle";
    pub const NEEDS_INPUT: &str = "needs_input";
    pub const AWAITING_FEEDBACK: &str = "awaiting_feedback";
    pub const BLOCKED: &str = "blocked";
    pub const DONE: &str = "done";
    pub const ERROR: &str = "error";
    pub const INTERRUPTED: &str = "interrupted";

    /// States from which reconciliation does *not* move a session to `interrupted`.
    pub const TERMINAL: &[&str] = &[DONE, ERROR, INTERRUPTED];

    /// Whether a state is terminal (the session is finished).
    pub fn is_terminal(state: &str) -> bool {
        TERMINAL.contains(&state)
    }
}

/// `worktrees.lease_state` values (`architecture.md` / worktree pool).
pub mod lease_state {
    pub const AVAILABLE: &str = "available";
    pub const LEASED: &str = "leased";
    pub const DIRTY: &str = "dirty";
    pub const RETIRED: &str = "retired";
}

/// Errors from the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("schema version {found} is newer than this build supports ({supported}); refusing to open")]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

/// Forward-only migrations. Index `i` migrates the schema from version `i` to `i+1`.
/// Never edit a shipped entry; add a new one.
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_init.sql"),
    include_str!("migrations/0002_sessions_cardless.sql"),
    include_str!("migrations/0003_knowledge_and_status_note.sql"),
    include_str!("migrations/0004_recipes.sql"),
    include_str!("migrations/0005_env_vault.sql"),
    include_str!("migrations/0006_artifacts_services.sql"),
    include_str!("migrations/0007_rounds_lan.sql"),
    include_str!("migrations/0008_gate_github.sql"),
];

/// The schema version this build understands (the number of migrations).
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// The SQLite store.
pub struct Store {
    conn: Mutex<Connection>,
    events_tx: broadcast::Sender<CardEvent>,
}

impl Store {
    /// Open (creating if needed) the store at `path`, applying pending migrations.
    pub fn open(path: &Path) -> Result<Store, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Open an in-memory store (tests).
    pub fn open_in_memory() -> Result<Store, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Store, StoreError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL for concurrent readers alongside the single writer (`data-model.md`).
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&conn)?;
        let (events_tx, _rx) = broadcast::channel(EVENT_BROADCAST_CAPACITY);
        Ok(Store { conn: Mutex::new(conn), events_tx })
    }

    /// Subscribe to committed card events (live `event.subscribe` tail).
    pub fn subscribe_events(&self) -> broadcast::Receiver<CardEvent> {
        self.events_tx.subscribe()
    }

    /// Append a card event outside any other mutation (dispatch bookkeeping).
    pub fn append_card_event(
        &self,
        card_id: &str,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<CardEvent, StoreError> {
        self.tx_events(|tx, events| append_event(tx, events, card_id, kind, payload))
    }

    /// Card events after `cursor` (exclusive), oldest first, capped at `limit`.
    /// A `None` cursor starts from the very beginning of the log.
    pub fn events_after(&self, cursor: Option<&str>, limit: i64) -> Result<Vec<CardEvent>, StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, card_id, kind, payload, ts FROM card_events \
             WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let cursor = cursor.unwrap_or("");
        let rows = stmt.query_map(params![cursor, limit], card_event_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The newest card-event cursor, or `None` when the log is empty.
    pub fn latest_event_cursor(&self) -> Result<Option<String>, StoreError> {
        let conn = self.lock();
        let cursor = conn
            .query_row("SELECT max(id) FROM card_events", [], |r| r.get::<_, Option<String>>(0))?;
        Ok(cursor)
    }

    // ---- shared helpers, visible to the submodules ----

    /// Lock the single connection. Held only for the duration of one statement group.
    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("store connection mutex poisoned")
    }

    /// Run `f` in one transaction; on commit, broadcast every event it appended.
    ///
    /// `f` appends events via [`append_event`], which pushes onto the shared vec so
    /// the broadcast happens after the commit succeeds, never before.
    pub(super) fn tx_events<T>(
        &self,
        f: impl FnOnce(&Transaction, &mut Vec<CardEvent>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut events: Vec<CardEvent> = Vec::new();
        let out = {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            let out = f(&tx, &mut events)?;
            tx.commit()?;
            out
        };
        for ev in &events {
            // A send error just means no subscribers; the event is already durable.
            let _ = self.events_tx.send(ev.clone());
        }
        Ok(out)
    }
}

/// Append a `card_events` row inside `tx` and record it for post-commit broadcast.
///
/// Every payload is run through the value-matching secret scrubber against the live
/// materialized-secret set before it is persisted or broadcast (`security.md` / Event
/// payloads and timelines: "the scrubber runs as defense in depth on payload writes").
/// This is a no-op when no session currently carries vault secrets.
pub(super) fn append_event(
    tx: &Transaction,
    events: &mut Vec<CardEvent>,
    card_id: &str,
    kind: &str,
    mut payload: serde_json::Value,
) -> Result<CardEvent, StoreError> {
    let registry = crate::secret::registry();
    if !registry.is_empty() {
        crate::secret::scrub_json(&mut payload, &registry.all_values());
    }
    let id = new_ulid().to_string();
    let ts = now_ms();
    let payload_str = if payload.is_null() { None } else { Some(payload.to_string()) };
    tx.execute(
        "INSERT INTO card_events (id, card_id, kind, payload, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, card_id, kind, payload_str, ts],
    )?;
    let event = CardEvent { id, card_id: card_id.to_string(), kind: kind.to_string(), payload, ts };
    events.push(event.clone());
    Ok(event)
}

/// A monotonic ULID for store-issued ids.
///
/// `Ulid::new()` randomizes the low bits, so two ids minted in the same millisecond
/// can sort out of insertion order. `card_events.id` is the resumable stream cursor
/// (`protocol.md` / event.subscribe), so ordering must match insertion; the shared
/// `Generator` increments within a millisecond to guarantee it.
pub(super) fn new_ulid() -> Ulid {
    static GEN: OnceLock<Mutex<Generator>> = OnceLock::new();
    let generator = GEN.get_or_init(|| Mutex::new(Generator::new()));
    let mut guard = generator.lock().expect("ulid generator poisoned");
    // Overflow within one millisecond is astronomically unlikely; fall back to a
    // random ULID rather than failing the mutation.
    guard.generate().unwrap_or_else(|_| Ulid::new())
}

/// Milliseconds since the Unix epoch.
pub(super) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Serialize check commands to the JSON stored in `projects.check_cmds`.
pub(super) fn check_cmds_to_json(cmds: &[CheckCmd]) -> Option<String> {
    if cmds.is_empty() {
        None
    } else {
        serde_json::to_string(cmds).ok()
    }
}

/// Map a `card_events` row to the wire `CardEvent`.
pub(super) fn card_event_from_row(row: &Row) -> rusqlite::Result<CardEvent> {
    let payload_str: Option<String> = row.get(3)?;
    let payload = payload_str
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    Ok(CardEvent {
        id: row.get(0)?,
        card_id: row.get(1)?,
        kind: row.get(2)?,
        payload,
        ts: row.get(4)?,
    })
}

/// Map a `projects` row to the wire `Project`.
pub(super) fn project_from_row(row: &Row) -> rusqlite::Result<Project> {
    let check_cmds_json: Option<String> = row.get("check_cmds")?;
    let check_cmds = check_cmds_json
        .and_then(|s| serde_json::from_str::<Vec<CheckCmd>>(&s).ok())
        .unwrap_or_default();
    Ok(Project {
        id: row.get("id")?,
        path: row.get("path")?,
        name: row.get("name")?,
        default_branch: row.get("default_branch")?,
        mode: row.get("mode")?,
        check_cmds,
        default_recipe: row.get("default_recipe")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Apply pending forward-only migrations; refuse a newer-than-known schema.
fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew { found: current, supported: SCHEMA_VERSION });
    }
    for version in current..SCHEMA_VERSION {
        let sql = MIGRATIONS[version as usize];
        // The DDL and the version bump commit atomically, so an interrupted migration
        // never leaves a half-applied schema. `version + 1` is our own integer, not
        // client input, so formatting it into the pragma is safe.
        conn.execute_batch(&format!(
            "BEGIN;\n{sql}\nPRAGMA user_version = {};\nCOMMIT;",
            version + 1
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
