//! Phase 0 control-message payloads (`protocol.md` / Message families).
//!
//! Only the handshake and the `session.*` subset are modelled here. Where Phase 0
//! diverges from the full spec it is called out inline:
//!
//! - `session.create` in `protocol.md` takes `{ card_id, harness, model?, effort?,
//!   worktree_id, env }` and resolves a worktree/brief through the dispatch layer.
//!   That layer (cards, worktrees, recipes) does not exist until later phases, so
//!   Phase 0 accepts the concrete spawn inputs (`cols`, `rows`, `cwd`, `command`)
//!   directly. `card_id`/`worktree_id` remain optional for forward compatibility.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::entities::{
    Agent, ArtifactMeta, Card, CardEvent, FeedbackItem, FindingInfo, GateRunInfo, GithubIssueInfo,
    LayoutWarning, NeedsYouItem, Project, RecipeInvalid, RecipeSummary, RecipeValidationError,
    ServiceInfo, SessionSummary,
};
use crate::snapshot::{CursorPos, StyledSnapshot};

/// The kind of client performing the handshake (`protocol.md` / Authentication).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Desktop,
    Agent,
    Mobile,
    /// The Concertmaster MCP server (`dflow-mcp`). It authenticates with an
    /// owner-scoped token but declares this kind so the daemon can *attribute* its
    /// actions - notably emitting `concertmaster_steered` when it calls
    /// `session.send_verified` (`phase6-mcp.md` merge-time request 1). Additive variant;
    /// an older daemon that does not know it simply treats the token's own scope.
    Mcp,
}

/// `auth.hello` request: the first frame a client must send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthHello {
    pub token: String,
    pub client: ClientKind,
    /// Protocol envelope versions the client supports; the daemon picks the newest
    /// it shares (`protocol.md` / Versioning).
    pub proto_versions: Vec<u8>,
}

/// `auth.welcome` response: granted scope and the chosen protocol version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthWelcome {
    pub proto_version: u8,
    /// The granted scope. Phase 0 mints only the root scope; per-task scoping is a
    /// later phase (`security.md` / Token architecture).
    pub scope: String,
    /// Human-readable daemon build string, useful in logs and the status bar.
    pub daemon_version: String,
}

/// `session.create` request. See the module note for the Phase 0 divergence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreate {
    /// Forward-compatible dispatch linkage; unused in Phase 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// Adapter name. Phase 0 understands `"powershell"` (and `"cmd"`); an explicit
    /// `command` overrides the harness default.
    pub harness: String,
    /// Optional configured launcher (name or id). When set, the daemon resolves the
    /// launcher's command + extra args and merges its extra env into `env`
    /// (`product.md` / Settings > Agents). An explicit `command` still wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// An optional first prompt for the New Session front door: it is recorded as the
    /// session preview and, once the composer is ready, submitted through verified
    /// submit (`adapters.md` / Verified submit; Phase 2 first-prompt auto-submit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
}

/// `session.create` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCreated {
    pub session_id: String,
    /// Whether a first prompt was accepted for verified auto-submit on this session.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub first_prompt_queued: bool,
}

/// `session.attach` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAttach {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
}

/// `session.attach` response: state replay for the client.
///
/// Carries both a structured styled snapshot (for the daemon's own heuristics and
/// future non-xterm clients) and `replay_base64`: the recent raw PTY bytes from the
/// scrollback ring. An xterm.js client writes `replay_base64` straight into the
/// terminal to reconstruct the exact screen and scrollback, then live binary output
/// frames continue from there. This is the persistence proof at the wire level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAttached {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
    pub cursor: CursorPos,
    pub snapshot: StyledSnapshot,
    /// Base64 of the recent raw PTY output held in the scrollback ring.
    pub replay_base64: String,
}

/// `session.detach` request. Detaching never kills the session (`architecture.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetach {
    pub session_id: String,
}

/// `session.kill` request: terminate the session and its whole process tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionKill {
    pub session_id: String,
}

/// `session.list` request (no fields).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionList {}

/// A compact fleet-table row (`protocol.md` / session.list).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub harness: String,
    pub cols: u16,
    pub rows: u16,
    /// Whether the child process is still running.
    pub alive: bool,
    /// Number of clients currently attached.
    pub attached: usize,
    /// Milliseconds since the Unix epoch at creation.
    pub created_at_ms: u64,
}

/// `session.list` response: the enriched fleet table (`protocol.md`).
///
/// Rows are `SessionSummary`, merging live PTY facts with the persisted session row
/// (card, project, state, title, elapsed), and include `interrupted` sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListResult {
    pub sessions: Vec<SessionSummary>,
}

/// `session.rename` request: set a session's tab label (`protocol.md`).
/// An empty `title` clears the label back to the client-generated default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRename {
    pub session_id: String,
    pub title: String,
}

/// `fleet.status {}` request: one fleet snapshot (`protocol.md` fleet.*).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FleetStatus {}

/// `fleet.status` response: the enriched session table plus open Needs You
/// items, highest score first. Gate runs join with the gate engine (M5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetStatusResult {
    pub sessions: Vec<SessionSummary>,
    pub needs_you: Vec<NeedsYouItem>,
}

/// `daemon.shutdown` request: graceful shutdown for tests and clean exits.
/// Requires the root scope. Not part of the public client protocol surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonShutdown {}

/// A generic `{ ok: true }` acknowledgement for verbs with no richer response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Simple {
    pub ok: bool,
}

impl Simple {
    pub fn ok() -> Self {
        Self { ok: true }
    }
}

// ---------------------------------------------------------------------------
// Phase 1 families: project.*, card.*, dispatch.*, event.* (`protocol.md`).
// All additive: existing envelope version and Phase 0 messages are unchanged.
// ---------------------------------------------------------------------------

/// `project.add { path }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAdd {
    pub path: String,
}

/// `project.add` response. `project_id` is the specced field; `project` is the full
/// entity, included so the client need not immediately re-list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAdded {
    pub project_id: String,
    pub project: Project,
}

/// `project.update { project_id, mode?, check_cmds?, default_recipe? }` request.
/// Absent fields are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectUpdate {
    pub project_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_cmds: Option<Vec<crate::entities::CheckCmd>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_recipe: Option<String>,
    /// The round schedule json (`{enabled, interval_minutes}`) for floor-check/general
    /// rounds (`product.md` / Concertmaster rounds). `Some("")` clears it (rounds off);
    /// `None` leaves it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds_schedule: Option<String>,
    /// The gardener-round schedule json, same shape (`knowledge.md` / gardener as a round
    /// type at M4). `Some("")` clears it; `None` leaves it unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gardener_schedule: Option<String>,
}

/// `project.update` response: the updated project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectUpdated {
    pub project: Project,
}

/// `project.list {}` request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectList {}

/// `project.list` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectListResult {
    pub projects: Vec<Project>,
}

/// `card.create { title, type, project_id?, dial_recipe?, brief? }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardCreate {
    pub title: String,
    #[serde(rename = "type")]
    pub card_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// Optional starting priority (additive; defaults to 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    /// Optional starting lane (additive; defaults to `inbox`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    /// A stable dedupe slug (`dflow card create --fingerprint`): stamped onto
    /// `origin_ref` so re-audits refresh rather than refile (`agent-cli.md`,
    /// `product.md` / onboarding audit). The daemon sets `origin_kind` from the
    /// caller's token (audit-scoped -> `audit`, else `manual`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// `card.create` response. `card_id` is the specced field; `card` is the full entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardCreated {
    pub card_id: String,
    pub card: Card,
    /// For an origin-keyed (audit fingerprint) create, the dedupe outcome (`data-model.md`
    /// / durable audit dismissals): `created` (new), `refreshed` (an existing finding
    /// updated in place), or `suppressed` (a dismissed finding left untouched, not
    /// refiled). Absent for a plain create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedupe: Option<String>,
}

/// `card.update { card_id, ... }` request. Absent fields are left unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardUpdate {
    pub card_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub card_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
}

/// `card.update`/`card.move` response: the updated card.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardResult {
    pub card: Card,
}

/// `card.move { card_id, column }` request. `column` is the board column the card
/// moves to; it maps to the DB `lane` column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardMove {
    pub card_id: String,
    pub column: String,
}

/// A `card.query` filter. All fields optional; omitted fields do not constrain.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub card_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
}

/// `card.query { filter }` request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CardQuery {
    #[serde(default)]
    pub filter: CardFilter,
}

/// `card.query` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardQueryResult {
    pub cards: Vec<Card>,
}

/// `card.get { card_id }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardGet {
    pub card_id: String,
    /// Max events to return (newest first); defaults applied by the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events_limit: Option<i64>,
    /// Return events strictly older than this cursor (for paging back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub events_before: Option<String>,
}

/// `card.get` response: the card plus its sessions, latest events, and Plan Studio
/// artifacts (`protocol.md`; `phase5-m3-ui.md` / Interpretation 2: artifacts arrive on
/// `card.get` as `ArtifactMeta[]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardGetResult {
    pub card: Card,
    pub sessions: Vec<SessionSummary>,
    pub events: Vec<CardEvent>,
    /// Plan/mockup/diagram artifacts registered on the card, newest first (empty when
    /// none). Additive: an older client ignores the field.
    #[serde(default)]
    pub artifacts: Vec<ArtifactMeta>,
}

/// `dispatch.start { card_id, recipe?, agent?, harness?, model?, effort? }` request.
///
/// Resolution precedence (`adapters.md` / dispatch resolves launcher first): explicit
/// `agent` (launcher name or id) > explicit `harness` (a same-named enabled launcher
/// if one exists, else the legacy built-in table) > project default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchStart {
    pub card_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// Configured launcher to dispatch through (name or id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Per-dispatch card-creation cap for the minted task token (`recipes.md` /
    /// budgets). `None` means unbudgeted; the audit recipe sets it at M3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_cards: Option<u32>,
    /// Per-dispatch note-creation cap for the minted task token (`knowledge.md` /
    /// audit note-budget caps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_notes: Option<u32>,
    /// Mint an audit-scoped token: cards it creates land in Inbox and it may never move
    /// their lanes (`security.md` / Audit sessions).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub audit: bool,
    /// Explicit acknowledgement for a `worktree: in_place` recipe: the caller confirms
    /// the session will edit the project checkout directly. Required IN ADDITION to the
    /// per-project privilege grant (`recipes.md` / implement.worktree).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ack_in_place: bool,
}

/// `dispatch.start` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchStarted {
    pub session_id: String,
    /// The leased worktree id, or empty for a `worktree: in_place` dispatch (no lease;
    /// the session runs in the project checkout itself).
    pub worktree_id: String,
    /// The session working directory: the leased worktree path, or the project root
    /// for a `worktree: in_place` dispatch.
    pub worktree_path: String,
    /// The adapter family the session runs under (`data-model.md` / sessions.harness).
    pub harness: String,
    /// The launcher name resolved for this dispatch, when one was used (null for the
    /// legacy built-in path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The flow recipe this dispatch resolved (`recipes.md`: dispatch resolves the
    /// recipe first; the name and version are also recorded in the dispatched event).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe_version: Option<u32>,
}

/// `session.resume { session_id }` request: relaunch an interrupted/ended session in
/// the same cwd/worktree with the harness resume flag (`architecture.md` / session
/// resume). The harness reloads its own transcript; a NEW session row is created and
/// linked via `resumed_from`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResume {
    pub session_id: String,
}

/// `session.resume` response: the new session id and the predecessor it resumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResumed {
    pub session_id: String,
    pub resumed_from: String,
    /// The harness-native ref used to resume (echoed for the UI/diagnostics).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<String>,
}

/// `dispatch.cancel { card_id }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchCancel {
    pub card_id: String,
}

/// `dispatch.cancel` response: how many live sessions were killed and the resulting
/// worktree lease state (`available` if returned clean, `dirty` if parked).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchCancelled {
    pub cancelled: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_state: Option<String>,
}

/// `event.subscribe { cursor? }` request. `cursor` is the ULID of the last seen
/// `card_events` row; the stream replays from just after it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventSubscribe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `event.subscribe` response acknowledging the subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscribed {
    pub ok: bool,
    /// The newest cursor the daemon knows right now (may be null if the log is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_cursor: Option<String>,
}

/// `event.ack { cursor }` request: the client's persisted bookmark.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventAck {
    pub cursor: String,
}

/// A streamed `event.card_event` message payload (server-initiated, no envelope id).
/// The `event` carries its own ULID `id`, which is the resumable cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCardEvent {
    pub event: CardEvent,
}

// ---------------------------------------------------------------------------
// Phase 1.5 family: agents.* (`protocol.md` / session.*, `product.md` /
// Settings > Agents). Configured launchers: detected or user-defined.
// ---------------------------------------------------------------------------

/// `agents.list {}` request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsList {}

/// `agents.list` / `agents.add` / `agents.update` response carrying launchers with
/// `detected_version`, `enabled`, `source`, and computed `caution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsListResult {
    pub agents: Vec<Agent>,
}

/// `agents.add { name, adapter, command, extra_args, extra_env }` request.
/// Creates a `source: custom` launcher (`protocol.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAdd {
    pub name: String,
    pub adapter: String,
    pub command: String,
    /// JSON array of strings; serde rejects any other shape as a bad request.
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// JSON object of string->string; serde rejects any other shape.
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
}

/// `agents.add` / `agents.update` single-launcher response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub agent: Agent,
}

/// `agents.update { id, ... }` request. `id` accepts a launcher id or name; absent
/// fields are left unchanged. `enabled` toggles the launcher on or off.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentUpdate {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// `agents.remove { id }` request. `id` accepts a launcher id or name. The daemon
/// refuses while a non-ended session references the launcher (disable instead).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRemove {
    pub id: String,
}

/// `agents.remove` response: the removed launcher's name, for a clean confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRemoved {
    pub ok: bool,
    pub removed: String,
}

/// `agents.detect {}` request: scan PATH for known CLIs (`protocol.md`). Never runs
/// automatically; the UI calls it explicitly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentsDetect {}

/// One CLI the PATH scan turned up this run (evidence for the UI/diagnostics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedCli {
    /// The CLI/adapter name (`claude`, `codex`, `opencode`, `cursor`, `pi`).
    pub name: String,
    /// The executable resolved on PATH (full path with its shim extension).
    pub command: String,
    /// Version parsed from `<command> --version`, when the probe succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether this run created a brand-new launcher for the CLI.
    pub created: bool,
}

/// `agents.detect` response: the CLIs found this run plus the refreshed launcher list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsDetected {
    pub found: Vec<DetectedCli>,
    pub agents: Vec<Agent>,
}

// ---------------------------------------------------------------------------
// M2 agent-CLI families: agent.*, session self-report, know.*, notify.* .
// These back the `dflow` binary (`agent-cli.md`, `knowledge.md`). Their surfaces
// are token-scoped: an agent-scoped connection resolves card/session/project from
// its per-task token, so the request payloads carry little or no addressing.
// ---------------------------------------------------------------------------

/// `agent.context {}`: the CLI's one read for bare `dflow` and `dflow card`. The
/// per-task token resolves the current card, session, and project.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentContext {}

/// `agent.context` response: everything bare `dflow` and `dflow card` render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_note: Option<String>,
    /// Acceptance criteria parsed from the brief; empty when none are recorded.
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// The project knowledge digest (index.md Digest section, 30-line capped), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Number of catalogued knowledge notes for the project (an AXI aggregate).
    #[serde(default)]
    pub knowledge_notes: u32,
}

/// `session.self_report { state, note? }`: tier-1 lifecycle self-report (`dflow
/// status`). The token resolves the session; `done` is a stage-advance request the
/// daemon arbitrates (`agent-cli.md` / Stage advancement arbitration).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfReport {
    /// `working | blocked | done` (the tier-1 self-report set).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `session.self_report` response: the recorded state and the arbitration outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfReportResult {
    /// The lifecycle state actually recorded.
    pub recorded: String,
    /// Whether a `done` request advanced the stage (`agent-cli.md` / Stage advancement
    /// arbitration: agent signals are inputs, recipe conditions are gates).
    #[serde(default)]
    pub advanced: bool,
    /// Whether a status note was stored alongside the state.
    #[serde(default)]
    pub note_set: bool,
    /// When a `done` request is gated, the missing condition (plan approval gating
    /// itself is deferred until Plan Studio; the recipe still names what comes next).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    /// A recipe-aware `next:` hint the daemon composes from the dispatch recipe's stage
    /// list, so the agent hears what the flow does after this stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// `session.set_note { note }`: set the session-strip status note (`dflow card note`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetNote {
    pub note: String,
}

/// `notify.forward { payload }`: the codex notify bridge (`dflow notify-forward`).
/// `payload` is the raw JSON codex hands its `notify` program; the daemon parses the
/// `agent-turn-complete` type and captures `thread-id` into `resume_ref`
/// (`adapters.md` / Resume-ref capture, codex row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyForward {
    pub payload: String,
}

/// `know.index {}`: digest + catalog counts (`dflow know`). For a root connection an
/// explicit `project_id` selects the project; an agent connection uses its token.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowIndex {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// One catalog line: a note type and how many notes carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowCatalogGroup {
    #[serde(rename = "type")]
    pub note_type: String,
    pub count: u32,
}

/// `know.index` response: the whole index at a glance (`knowledge.md` / `dflow know`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowIndexResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Line count of the digest (an AXI aggregate: agents budget their reads).
    #[serde(default)]
    pub digest_lines: u32,
    #[serde(default)]
    pub catalog: Vec<KnowCatalogGroup>,
    #[serde(default)]
    pub total_notes: u32,
}

/// `know.find { query, type? }`: substring/tag search (`dflow know find`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowFind {
    pub query: String,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub note_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// One `know.find` hit: id (path minus `.md`), type, and description.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowNoteHit {
    pub id: String,
    #[serde(rename = "type")]
    pub note_type: String,
    pub description: String,
}

/// `know.find` response: the matching notes, id-sorted for stable output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowFindResult {
    pub notes: Vec<KnowNoteHit>,
}

/// `know.get { id, full }`: print one note (`dflow know get`), truncated unless `full`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowGet {
    pub id: String,
    #[serde(default)]
    pub full: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// One note's rendered content (`know.get`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowNote {
    pub id: String,
    #[serde(rename = "type")]
    pub note_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The note body (frontmatter stripped), truncated to a line cap unless `full`.
    pub body: String,
    /// Whether the body was truncated (a size hint + `--full` escape hatch follow).
    #[serde(default)]
    pub truncated: bool,
    /// Total body lines, so the CLI can print a size hint.
    #[serde(default)]
    pub total_lines: u32,
}

/// `know.get` response: the note, or `None` when the id does not resolve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowGetResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<KnowNote>,
}

/// `know.add { type, title, body, tags? }`: create or replace a note (`dflow know
/// add`). Provenance `card:` is stamped from the token; idempotent per id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowAdd {
    #[serde(rename = "type")]
    pub note_type: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// `know.add` response: the note id, its path, and whether it was newly created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowAddResult {
    pub id: String,
    pub path: String,
    /// True on first create, false when an existing id was replaced.
    #[serde(default)]
    pub created: bool,
}

// ---------------------------------------------------------------------------
// recipe.* family (`protocol.md` / recipe.*, `recipes.md`). Recipes are file-truth;
// these verbs list/get/validate/install them and record the privilege grants a
// privileged recipe needs (`security.md` / Recipe trust tiers). The full parsed recipe
// travels as an opaque `parsed` JSON value so this crate need not mirror the schema.
// ---------------------------------------------------------------------------

/// `recipe.list {}`: bundled + user + project recipes with their winning source. An
/// optional `project_id` includes that project's project-scoped recipes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecipeList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// `recipe.list` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeListResult {
    pub recipes: Vec<RecipeSummary>,
    /// Files that failed to parse, surfaced rather than dropped.
    #[serde(default)]
    pub invalid: Vec<RecipeInvalid>,
}

/// `recipe.get { name }`: the resolved recipe's summary and full parsed structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeGet {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

/// `recipe.get` response: the summary plus the resolved recipe as an opaque JSON value,
/// or a resolution error when the name does not resolve.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeGetResult {
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RecipeSummary>,
    /// The fully inheritance-resolved recipe as JSON (`serde` of the engine's `Recipe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,
    /// A resolution/validation error when `found` is false.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<RecipeValidationError>,
}

/// `recipe.validate { content }`: parse arbitrary recipe text and report precise errors
/// without installing it (`recipes.md` / Validation and safety).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeValidate {
    pub content: String,
    /// Name hint (usually the intended file stem) used only when the front matter omits
    /// `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `recipe.validate` response: valid + parsed + trust classification, or precise errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeValidateResult {
    pub valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,
    /// `standard | privileged` when valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_tier: Option<String>,
    /// One-line descriptions of each elevated capability (privileged recipes).
    #[serde(default)]
    pub elevations: Vec<String>,
    #[serde(default)]
    pub errors: Vec<RecipeValidationError>,
}

/// `recipe.install { source, scope }`: copy a recipe file into a scope and validate it
/// (`protocol.md`). `source` is a filesystem path; `content` may carry the text inline
/// when the caller already has it (a `url` source is a later concern).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeInstall {
    pub source: String,
    /// `user | project`.
    pub scope: String,
    /// Required when `scope` is `project`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Inline recipe text; when present, `source` is only the target file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// `recipe.install` response: where it landed and its resulting trust tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeInstalled {
    pub name: String,
    pub scope: String,
    pub path: String,
    pub trust_tier: String,
}

/// `recipe.grant { project_id, recipe_name }`: record per-project consent for a
/// privileged recipe (`security.md`). The grant captures the current file hash, so a
/// later edit invalidates it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeGrant {
    pub project_id: String,
    pub recipe_name: String,
}

/// `recipe.grant` response: what was granted and the hash it is bound to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeGranted {
    pub project_id: String,
    pub recipe_name: String,
    pub recipe_hash: String,
    /// One-line descriptions of exactly what was elevated by this grant.
    #[serde(default)]
    pub elevations: Vec<String>,
}

/// `recipe.revoke_grant { project_id, recipe_name }`: revoke a per-project grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeRevokeGrant {
    pub project_id: String,
    pub recipe_name: String,
}

/// `recipe.revoke_grant` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeGrantRevoked {
    pub revoked: bool,
}

// ---------------------------------------------------------------------------
// env.* family (`protocol.md` / env.*, `environments.md`). The per-project vault:
// values are write-only from clients (`set`), reads are materialization-only. `list`
// returns names and kinds, never values. `materialize`/`cleanup` are daemon-internal,
// exposed for diagnostics. `import` ingests an existing `.env` file.
// ---------------------------------------------------------------------------

/// `env.set { project_id, key, value, kind, target? }` (`protocol.md`). Values are
/// write-only from clients; a response never echoes the value back.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSet {
    pub project_id: String,
    pub key: String,
    /// The plaintext value to seal at rest. Write-only: never returned by any verb.
    pub value: String,
    /// `secret | var | file`.
    pub kind: String,
    /// Required for `kind: file`: the relative target path template in the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// One vault entry as it appears on the wire: names and kinds only, never a value
/// (`protocol.md`: `env.list` -> names and kinds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvEntryInfo {
    pub key: String,
    /// `secret | var | file`.
    pub kind: String,
    /// For `file` entries: the relative target path template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Per-entry version, bumped on rotate.
    #[serde(default)]
    pub version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

/// `env.set` response: the entry's metadata (never its value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSetResult {
    pub entry: EnvEntryInfo,
    /// Whether the backing credential store provides real at-rest encryption. `false`
    /// on the non-Windows developer stub, so a client can warn (`environments.md`).
    #[serde(default)]
    pub secure_at_rest: bool,
}

/// `env.list { project_id }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvList {
    pub project_id: String,
}

/// `env.list` response: names and kinds only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvListResult {
    pub entries: Vec<EnvEntryInfo>,
}

/// `env.delete { project_id, key }` request (rotation/removal from Settings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDelete {
    pub project_id: String,
    pub key: String,
}

/// `env.delete` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDeleted {
    pub deleted: bool,
}

/// `env.materialize { worktree_id }` request (daemon-internal; exposed for diagnostics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvMaterialize {
    pub worktree_id: String,
}

/// `env.materialize` response: counts and the file targets written, never any value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvMaterialized {
    pub vars: usize,
    pub secrets: usize,
    /// Relative target paths of the `file` entries written into the worktree.
    #[serde(default)]
    pub files: Vec<String>,
}

/// `env.cleanup { worktree_id }` request (daemon-internal; exposed for diagnostics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvCleanup {
    pub worktree_id: String,
}

/// `env.cleanup` response: how many materialized secret files were shredded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvCleaned {
    pub shredded: usize,
}

/// `env.import { project_id, path }` request: parse a `.env` file into vault entries
/// (`environments.md` / Import assist).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvImport {
    pub project_id: String,
    /// Filesystem path to the `.env`-style file to parse.
    pub path: String,
}

/// `env.import` response: what the import did (never any value).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvImportResult {
    /// `(key, kind)` for every entry ingested, in file order.
    pub entries: Vec<EnvEntryInfo>,
    pub imported: usize,
    pub secrets: usize,
    pub vars: usize,
    /// Lines that could not be parsed as `KEY=VALUE` (blank/comment lines are ignored).
    #[serde(default)]
    pub skipped: Vec<String>,
}

// ---------------------------------------------------------------------------
// session.peek and session.send_verified (`protocol.md` / session.*; `phase6-mcp.md`
// merge-time requests). Peek is a read-only scrubbed screen capture; send_verified is
// the guarded steering verb the Concertmaster drives.
// ---------------------------------------------------------------------------

/// `session.peek { session_id, lines? }` request: a read-only, bounded, scrubbed
/// plain-text screen capture that never resizes the PTY (`phase6-mcp.md` request 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPeek {
    pub session_id: String,
    /// Max lines of the visible screen to return (daemon clamps to a sane range).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
}

/// `session.peek` response: the bounded, scrubbed screen text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPeeked {
    pub session_id: String,
    /// The number of lines actually returned.
    pub lines: u32,
    /// The visible-screen tail as plain text, with known secret values redacted.
    pub text: String,
}

/// `session.send_verified { session_id, text, submit }` request (`protocol.md` /
/// session.send_verified; `adapters.md` / Verified submit). Types the text into the
/// composer and verifies submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendVerified {
    pub session_id: String,
    pub text: String,
    #[serde(default)]
    pub submit: bool,
}

/// `session.send_verified` response: whether the composer accepted the text and how
/// many attempts it took.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendVerifiedResult {
    pub submitted: bool,
    pub attempts: u32,
}

// ---------------------------------------------------------------------------
// auth.mint_concertmaster (`security.md` / Concertmaster capability scope). A daemon
// verb that mints a scoped Concertmaster token: read + steer + knowledge surface, with
// vault, kill, and merge-class verbs excluded. Owner-scope only.
// ---------------------------------------------------------------------------

/// `auth.mint_concertmaster {}` request: mint a Concertmaster-scoped token (root only).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MintConcertmaster {}

/// `auth.mint_concertmaster` response: the scoped token plus the excluded capabilities,
/// so a caller can display exactly what the profile withholds (`security.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcertmasterMinted {
    pub token: String,
    /// Human-readable names of the excluded verb classes (vault, kill, merge, push, ...).
    #[serde(default)]
    pub excludes: Vec<String>,
}

// ---------------------------------------------------------------------------
// artifact.* family (`protocol.md` / artifact.*, `plan-studio.md`). The Plan Studio
// review loop: an agent registers a plan HTML artifact, the daemon serves it over
// signed loopback URLs, the human annotates in the review chrome and submits feedback
// batches, and the agent long-polls for the queued batch. Shapes match the UI's
// contract (`phase5-m3-ui.md` / Interpretations).
// ---------------------------------------------------------------------------

/// `artifact.register { card_id?, path, kind? }` (from `dflow plan open`). An agent
/// connection omits `card_id` (its token resolves the card); a root client passes it.
/// `kind` defaults to `plan`. Idempotent per `(card, path)`: re-registering the same
/// path is a revision (new `revised_doc_id`, bumped round).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRegister {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    /// Absolute path to the self-contained HTML the daemon serves (the CLI resolves it).
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// `artifact.register` response: the artifact metadata plus reviewer guidance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRegistered {
    pub artifact: ArtifactMeta,
    /// Whether this register was a revision of an existing artifact (vs. a first open).
    #[serde(default)]
    pub revised: bool,
    /// Human-facing guidance: where the human reviews it (the app's Plan tab).
    pub review_hint: String,
}

/// `artifact.get { artifact_id }` (from the desktop): the metadata, a fresh short-lived
/// signed URL to point the sandboxed iframe at, and the latest layout audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactGet {
    pub artifact_id: String,
}

/// `artifact.get` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactGetResult {
    pub artifact: ArtifactMeta,
    /// A short-lived signed URL (`security.md` / Artifact sandbox); the iframe holds a
    /// capability, never a bearer token. The desktop re-gets to re-sign near expiry.
    pub signed_url: String,
    /// Epoch milliseconds at which `signed_url` expires.
    pub expires_at: i64,
    /// The latest layout-audit findings on the artifact (empty when clean/unaudited).
    #[serde(default)]
    pub layout_warnings: Vec<LayoutWarning>,
}

/// `artifact.feedback.submit { artifact_id, round, items, layout_warnings }` (from the
/// review chrome; `phase5-m3-ui.md` / Interpretation 3). Approve is modeled as an
/// `{ kind: "action", action: "approve_plan" }` item in the one item list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmit {
    pub artifact_id: String,
    /// The round the UI is submitting for (advisory; the daemon uses the artifact round
    /// when 0).
    #[serde(default)]
    pub round: u32,
    #[serde(default)]
    pub items: Vec<FeedbackItem>,
    /// The layout-audit findings the chrome computed; they land on the artifact record
    /// and flow back through the poll (`plan-studio.md` / Layout audit gate).
    #[serde(default)]
    pub layout_warnings: Vec<LayoutWarning>,
}

/// `artifact.feedback.submit` response (`phase5-m3-ui.md` / Interpretation 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSubmitResult {
    pub ok: bool,
    pub round: u32,
    /// The revise-in-place nonce, echoed so the UI can swap the frame (null: no revision
    /// produced by this submit; the daemon may instead stream an event).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_doc_id: Option<String>,
    pub next_step: String,
}

/// `artifact.feedback.poll { artifact_id?, wait? }` (from `dflow plan poll`). An agent
/// connection omits `artifact_id` (its token resolves the card's active plan artifact);
/// a root client may pass it. `wait` (default true) enables the bounded long-poll.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedbackPoll {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default = "default_true")]
    pub wait: bool,
}

fn default_true() -> bool {
    true
}

/// `artifact.feedback.poll` response (`agent-cli.md` / `dflow plan poll`;
/// `plan-studio.md` / Feedback payload). Carries a queued batch, or `pending` + re-poll
/// guidance, or `ended` + `next_step`; `next_step` is always present, and feedback is
/// never lost (an un-delivered batch persists until a poll consumes it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackPollResult {
    pub artifact_id: String,
    /// The round these items belong to (0 when none were delivered).
    #[serde(default)]
    pub round: u32,
    #[serde(default)]
    pub items: Vec<FeedbackItem>,
    #[serde(default)]
    pub layout_warnings: Vec<LayoutWarning>,
    /// True when the review ended (approved or ended): the agent stops polling.
    #[serde(default)]
    pub ended: bool,
    /// True when nothing is queued yet and the review is still open (safe to re-poll).
    #[serde(default)]
    pub pending: bool,
    /// True when the ending was a first-class plan approval.
    #[serde(default)]
    pub approved: bool,
    /// The artifact status after this poll (`open | awaiting_feedback | approved | ended`).
    pub status: String,
    /// The agent's most likely next step (always present, `agent-cli.md` design rule 6).
    pub next_step: String,
}

// ---------------------------------------------------------------------------
// service.* family (`data-model.md` / services, `environments.md`). Per-project local
// services the daemon starts per worktree at dispatch, with the port broker allocating
// real free ports injected as DFLOW_PORT_<NAME>.
// ---------------------------------------------------------------------------

/// `service.add { project_id, name, cmd, scope?, ports?, required? }`: declare a local
/// service (or replace one by name).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceAdd {
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    /// `per_worktree | shared` (defaults to `per_worktree`; shared is M4+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Named port declarations (e.g. `["HTTP","INSPECTOR"]`); each becomes a free port
    /// injected as `DFLOW_PORT_<NAME>` and substituted for `{DFLOW_PORT_<NAME>}` in cmd.
    #[serde(default)]
    pub ports: Vec<String>,
    /// A failed required service parks the card; defaults to true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// `service.add` single-service response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResult {
    pub service: ServiceInfo,
}

/// `service.list { project_id }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceList {
    pub project_id: String,
}

/// `service.list` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceListResult {
    pub services: Vec<ServiceInfo>,
}

/// `service.remove { project_id, name }` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRemove {
    pub project_id: String,
    pub name: String,
}

/// `service.remove` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRemoved {
    pub removed: bool,
}

/// The structured `detail` a dispatch consent error carries, so the UI can render a
/// consent flow directly from the error (`recipes.md`, `security.md`). Serialized into
/// `ProtocolError.detail` as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentRequired {
    pub recipe_name: String,
    pub project_id: String,
    pub recipe_hash: String,
    pub trust_tier: String,
    /// One-line descriptions of each elevated capability the human must approve.
    pub elevations: Vec<String>,
    /// Why consent is needed now: `no_grant` or `hash_changed`.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// round.* family (`product.md` / Concertmaster rounds; M4). A round is a headless
// Concertmaster-scoped session with a built-in brief and an escalation-only output
// contract: it files AT MOST ONE deduplicated Needs You digest via `round.digest`,
// and emits `round_completed`. Re-running a round type for the same scope dedupes.
// ---------------------------------------------------------------------------

/// `round.start { round_type, project_id?, agent?, harness? }` request. `round_type` is
/// `floor_check` (cross-card synthesis, silence/drift detection) or `garden` (the
/// Knowledge Gardener as a round type). `project_id` scopes the round to one project;
/// omitted means a global (cross-project) round. `agent`/`harness` pick the launcher;
/// absent, the daemon uses the configured Concertmaster launcher (or a default shell).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundStart {
    pub round_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

/// `round.start` response: the headless session plus its round card (which anchors the
/// round timeline and its single Needs You digest).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundStarted {
    pub session_id: String,
    /// The round card that anchors the digest + `round_completed` event.
    pub round_card: String,
    pub round_type: String,
    /// `project_id` for a scoped round, or `"all"` for a global round.
    pub scope: String,
}

/// `round.digest { body, findings? }` request (`dflow round digest --body`). Called by
/// the round agent (over its round-scoped token) to file the round one escalation
/// digest against its own round card. Idempotent per round: a second call updates the
/// same Needs You item in place, honoring the at-most-one contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundDigest {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<u32>,
}

/// `round.digest` response: the round card, the findings count, and whether this call
/// updated an existing digest (dedupe/re-run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundDigestResult {
    pub round_card: String,
    pub findings: u32,
    /// `true` when a digest for this round already existed (re-run/second call).
    pub deduped: bool,
}

// ---------------------------------------------------------------------------
// needs_you.* family. The Needs You queue is also carried in `fleet.status`, but the
// phone attention surface (`mobile.md`) needs a first-class list + resolve pair, so a
// phone-scoped client can drain the queue without the full fleet snapshot.
// ---------------------------------------------------------------------------

/// `needs_you.list {}` request: the open Needs You queue, highest score first.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NeedsYouList {}

/// `needs_you.list` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsYouListResult {
    pub items: Vec<NeedsYouItem>,
}

/// `needs_you.resolve { card_id, dedupe_key }` request: resolve one item (approvals,
/// dismissals). `resolved_by` is stamped daemon-side from the connection scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsYouResolve {
    pub card_id: String,
    pub dedupe_key: String,
}

/// `needs_you.resolve` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsYouResolved {
    pub resolved: bool,
}

// ---------------------------------------------------------------------------
// daemon.lan.* family (`security.md` / Remote access trust model; M6). The opt-in LAN
// listener: a SECOND listener (separate from loopback, off by default) that serves the
// mobile PWA at /m and the same WS protocol with phone-scoped capability tokens. QR
// pairing mints a phone token and returns the exact `mobile.md` pairing payload.
// ---------------------------------------------------------------------------

/// `daemon.lan.enable { port? }` request: bind the LAN listener (persisted). Omitting
/// `port` reuses the last persisted port, else a default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanEnable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// `daemon.lan.disable {}` request: stop the LAN listener (persisted off).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanDisable {}

/// `daemon.lan.status {}` request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanStatus {}

/// `daemon.lan.status` / `daemon.lan.enable` response: the listener state, the honest
/// no-TLS-on-LAN caveat text (`security.md`), the reachable `/m` URLs, and the live
/// phone pairings for the Settings revocation list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanState {
    pub enabled: bool,
    /// Whether a listener is currently bound (enabled but failed-to-bind reads false).
    pub bound: bool,
    /// The bound port (0 when not bound).
    pub port: u16,
    /// The reachable `http://<lan-ip>:<port>/m` URLs (one per detected LAN interface).
    #[serde(default)]
    pub lan_urls: Vec<String>,
    /// The honest security posture shown when the listener is enabled (`security.md`:
    /// LAN v1 ships without TLS; the capability token is the gate; trusted network only).
    pub caveat: String,
    /// Live phone pairings (id + label + timestamps), never the tokens themselves.
    #[serde(default)]
    pub phones: Vec<PhonePairing>,
}

/// One live phone pairing in `LanState.phones` (the token is never included).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhonePairing {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<i64>,
}

/// `daemon.lan.pair { name? }` request: mint a phone-scoped capability token and build
/// the QR pairing payload. Loopback-callable only (the desktop renders the QR).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LanPair {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `daemon.lan.pair` response: the exact `mobile.md` pairing payload plus a ready-to-
/// encode QR URL string. The token appears only here (in the URL fragment), never in a
/// server request line or log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanPairing {
    /// The per-device revocation target for this pairing.
    pub token_id: String,
    /// The QR string per `mobile.md`: `http://<lan-ip>:<port>/m#pair=<base64url(JSON)>`.
    pub pair_url: String,
    /// The decoded pairing payload the PWA parses (`{ url, token, name }`).
    pub payload: PairingPayload,
}

/// The pairing payload the PWA fragment carries (`mobile.md` / pairing payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingPayload {
    /// The phone WS endpoint: `ws://<lan-ip>:<port>/ws`.
    pub url: String,
    /// The phone-scoped capability token.
    pub token: String,
    /// An optional daemon/host label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `daemon.lan.revoke { token_id }` request: revoke one phone pairing (`security.md` /
/// Per-device revocation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanRevoke {
    pub token_id: String,
}

/// `daemon.lan.revoke` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanRevoked {
    pub revoked: bool,
}

// ---------------------------------------------------------------------------
// github.* family (`roadmap.md` M5.1-2, `product.md` / Card sources: GitHub issue
// import, `gate.md` / GitHub integration). Read-only import + auth reporting; the
// gh CLI is the transport (dflow-core::github). Push/PR/merge live in the gate/ship
// path, not here.
// ---------------------------------------------------------------------------

/// `github.auth.status {}`: report gh presence/auth without running an OAuth flow
/// (`roadmap.md` M5.1). Never mutates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubAuthStatus {}

/// `github.auth.status` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubAuthResult {
    /// Whether the `gh` CLI is installed and runnable.
    pub present: bool,
    /// Whether `gh` reports an authenticated account.
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The `owner/name` of the repo at the project path, when gh can read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// The issue-selection filter shared by preview and import (`product.md`: assignee,
/// label, and milestone filters, or a curated set of explicit numbers; never an
/// unfiltered firehose).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GithubIssueFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    /// A curated set of explicit issue numbers; when set, the other filters are ignored.
    #[serde(default)]
    pub numbers: Vec<u64>,
    /// `open` (default) | `closed` | `all`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `github.issues.preview { project_id, filter }`: list issues that WOULD be imported,
/// with a per-issue dedupe status, without creating any card (read-only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssuesPreview {
    pub project_id: String,
    #[serde(default)]
    pub filter: GithubIssueFilter,
}

/// One previewed issue plus its dedupe status against the board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssuePreview {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub url: String,
    /// `new` (no card yet) | `tracked` (a card already imports it) | `dismissed`
    /// (a card exists but was dismissed, so import would suppress).
    pub dedupe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_card_id: Option<String>,
}

/// `github.issues.preview` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssuesPreviewResult {
    /// `owner/name` of the resolved repo.
    pub repo: String,
    pub issues: Vec<GithubIssuePreview>,
}

/// `github.issues.import { project_id, filter, dial_recipe? }`: create/refresh origin
/// cards for the matching issues (`product.md`: one issue, one card; re-import refreshes
/// but respects local lane moves; deduplicates on re-import).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssuesImport {
    pub project_id: String,
    #[serde(default)]
    pub filter: GithubIssueFilter,
    /// The dial recipe stamped on newly created cards, when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_recipe: Option<String>,
}

/// One import outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubImportResult {
    pub number: u64,
    pub title: String,
    pub card_id: String,
    /// `created | refreshed | suppressed` (`data-model.md` / OriginUpsert).
    pub outcome: String,
}

/// `github.issues.import` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssuesImportResult {
    pub repo: String,
    pub results: Vec<GithubImportResult>,
}

/// `github.issue.get { card_id }`: the issue snapshot for an origin card's Issue tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssueGet {
    pub card_id: String,
}

/// `github.issue.get` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssueGetResult {
    pub issue: GithubIssueInfo,
}

// ---------------------------------------------------------------------------
// gate.* family (`gate.md` / Pipeline, Ship; `data-model.md` / gate_runs, findings).
// The verification gate: checks -> adversarial review -> autofix -> escalation ->
// ship. Owner scope drives it; the agent-side `finding.add` files findings.
// ---------------------------------------------------------------------------

/// `gate.run { card_id, head_sha?, author_harness?, reviewer_harness? }`: start a gate
/// run for a card. The pipeline runs asynchronously; progress streams as card events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRun {
    pub card_id: String,
    /// The commit under test; defaults to the card's authoring worktree HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// The implementing harness (for the reviewer-differs check); defaults to the card's
    /// latest session harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_harness: Option<String>,
    /// Override the recipe's reviewer harness (`different` or a concrete adapter name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_harness: Option<String>,
}

/// `gate.run` response: the run id and its status at kickoff (`running`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRunStarted {
    pub gate_run_id: String,
    pub status: String,
    /// The resolved gate strictness (`full | checks_only | none`).
    pub strictness: String,
}

/// `gate.status { card_id }`: the latest gate run plus its findings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateStatus {
    pub card_id: String,
}

/// `gate.status` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateStatusResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<GateRunInfo>,
    #[serde(default)]
    pub findings: Vec<FindingInfo>,
}

/// `gate.resolve_finding { finding_id, resolution }`: the human's escalation decision
/// (`gate.md` / Escalation: approve / fix / skip). `resolution` is `accepted | fixed |
/// skipped`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateResolveFinding {
    pub finding_id: String,
    pub resolution: String,
}

/// A single-finding response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingResult {
    pub finding: FindingInfo,
}

/// `gate.ship { card_id }`: push the passed branch and open a PR (pr mode), or perform an
/// approved local fast-forward merge (local_only mode) (`gate.md` / Ship, Project modes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateShip {
    pub card_id: String,
}

/// `gate.ship` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateShipResult {
    /// `pr | local_merge | none`.
    pub mode: String,
    pub pushed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// A human-readable outcome line (also used when gh is absent and PR mode degrades).
    pub message: String,
}

/// `gate.merge { card_id, method? }`: watch CI via `gh pr checks`, merge (squash default),
/// then prove the work landed before teardown (`gate.md` / Ship, Teardown safety).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMerge {
    pub card_id: String,
    /// `squash` (default) | `merge` | `rebase`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
}

/// `gate.merge` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateMergeResult {
    pub merged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<i64>,
    /// Whether the teardown landed-work proof passed (HEAD contained in the default
    /// branch); a false value means the worktree parked dirty instead of returning.
    pub landed: bool,
    pub message: String,
}

/// `finding.add { severity, body, category?, evidence? }` (agent scope): a reviewer
/// session files a structured finding against its active gate run (`gate.md` /
/// Adversarial review: "produces structured findings via `dflow finding add`").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingAdd {
    /// `blocker | major | minor`.
    pub severity: String,
    pub body: String,
    /// `mechanical` (safe-mechanical -> autofix) | `intent` (-> escalation). Defaults to
    /// `intent` (the conservative route: a human sees it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// An optional evidence pointer (a failing scenario, a rule citation, a log path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// `finding.add` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingAddResult {
    pub finding_id: String,
    pub gate_run_id: String,
    pub severity: String,
    pub category: String,
}
