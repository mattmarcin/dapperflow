//! Wire entity DTOs shared across families (`data-model.md`, `protocol.md`).
//!
//! Field names match `data-model.md` exactly, because a parallel UI agent codes
//! against these as the contract. The engine (`dflow-core`) constructs these
//! directly from SQLite rows; the daemon serializes them onto the wire.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One registered check command for the gate (`projects.check_cmds` json array).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckCmd {
    pub name: String,
    pub cmd: String,
}

/// A project (git repo root) in the registry (`data-model.md` / projects).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub path: String,
    pub name: String,
    pub default_branch: String,
    /// `pr | local_only`.
    pub mode: String,
    /// The gate's check commands; empty when unset.
    #[serde(default)]
    pub check_cmds: Vec<CheckCmd>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_recipe: Option<String>,
    /// Epoch milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
}

/// A board card (`data-model.md` / cards).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// `feature | bug | chore | test | investigation`.
    #[serde(rename = "type")]
    pub card_type: String,
    pub title: String,
    /// The board column. The DB column is named `lane` (`column` is an SQLite
    /// keyword); the wire entity keeps the `data-model.md` name `lane`.
    pub lane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dial_recipe: Option<String>,
    pub priority: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// `manual | github_issue | concertmaster | ...`.
    pub origin_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_ref: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One entry in a card's event log (`data-model.md` / card_events).
///
/// The `id` is a ULID and doubles as the resumable stream cursor
/// (`protocol.md` / event.subscribe).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardEvent {
    pub id: String,
    pub card_id: String,
    /// Taxonomy in `data-model.md`; unknown kinds must be preserved, never dropped.
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    /// Epoch milliseconds.
    pub ts: i64,
}

/// An enriched fleet-table row (`protocol.md` / session.list): the live PTY facts
/// merged with the persisted session row (card, project, lifecycle state, title).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    pub harness: String,
    /// The configured launcher name this session was dispatched through, when known
    /// (`data-model.md` / sessions.agent_id joined to agents.name). Distinguishes two
    /// launchers in the same adapter family (claude vs cc-alt) in the fleet table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The launcher id recorded on the session row (`sessions.agent_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// User-renamable tab label; null means the client generates one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent's last tier-1 status note (`dflow status`/`dflow card note`), which
    /// the board's session strip subtitles (`product.md` / session strip). Null until
    /// the agent self-reports one (M2 tier-1 signals).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_note: Option<String>,
    /// Lifecycle state (`adapters.md`); `interrupted` for a dead-PTY session.
    pub state: String,
    /// Whether a live PTY process backs this session right now.
    pub alive: bool,
    /// Milliseconds since the session was created.
    pub elapsed_ms: u64,
    /// Harness-native resume id, when captured (null until known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_ref: Option<String>,
    /// Preview of the first prompt, for the Projects view session list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
    /// Epoch milliseconds at creation.
    pub created_at_ms: u64,
}

/// A configured agent launcher (`data-model.md` / agents, `product.md` /
/// Settings > Agents). A launcher pairs an adapter family's behavior with the
/// user's own command, default arguments, and environment; detection creates them
/// for CLIs found on PATH, and users add custom ones (the canonical `cc-alt`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    /// Display name, unique across launchers (e.g. `claude`, `cc-alt`).
    pub name: String,
    /// Behavior family: `claude | codex | opencode | cursor | pi | custom`.
    pub adapter: String,
    /// Base executable launched for this agent.
    pub command: String,
    /// Default arguments appended at every launch (`[]` when unset).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Extra environment merged into the launch env, launcher wins (`{}` when unset).
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
    /// How the launcher was created: `detected` (PATH scan) or `custom` (user).
    pub source: String,
    /// Version captured from `<command> --version` at detection, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_version: Option<String>,
    /// Whether the launcher is offered in pickers and dispatchable.
    pub enabled: bool,
    /// Computed (not stored): `extra_args` weaken safety, so the UI styles it with a
    /// caution badge (`product.md` / Settings > Agents, extra-args caution styling).
    /// The danger list lives in `dflow_core::agents` (`caution`).
    #[serde(default)]
    pub caution: bool,
}

/// A recipe as summarized for `recipe.list`/`recipe.get` (`protocol.md` / recipe.*,
/// `recipes.md`). The full parsed structure travels as an opaque `parsed` JSON value on
/// the get/validate responses, so this crate never has to mirror the engine's schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeSummary {
    pub name: String,
    /// `bundled | user | project`.
    pub scope: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `standard | privileged` (`security.md` / Recipe trust tiers).
    pub trust_tier: String,
    /// The winning file's path; null for a bundled recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Less-specific scopes shadowed by this winner, so the UI shows which file won.
    #[serde(default)]
    pub shadowed_scopes: Vec<String>,
    /// One-line descriptions of each elevated capability (empty for standard recipes).
    #[serde(default)]
    pub elevations: Vec<String>,
}

/// A recipe file that failed to parse, surfaced so a broken file is visible in the list
/// rather than silently missing (`recipes.md` / Validation and safety).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeInvalid {
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub name_hint: String,
    pub error: RecipeValidationError,
}

/// One precise recipe validation error, with the offending source line when known.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeValidationError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

/// A plan/mockup/diagram artifact as summarized for `card.get` and `artifact.*`
/// (`plan-studio.md`, `phase5-m3-ui.md` / Interpretations 2). Field names match the
/// UI's `ArtifactMeta` exactly, because the Plan tab codes against this shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMeta {
    pub id: String,
    pub card_id: String,
    /// `plan | mockup | diagram | finding_review`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The serving identity the artifact HTTP endpoint signs (`phase5-m3-ui.md`).
    pub doc_id: String,
    /// The revise-in-place nonce; the iframe reloads when it changes (null before the
    /// first revision).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_doc_id: Option<String>,
    pub round: u32,
    /// `open | awaiting_feedback | approved | ended`.
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A quote-anchored text selection (`plan-studio.md` / Review chrome capabilities). The
/// **quote is the load-bearing anchor**, matched whitespace-normalized; `start`/`end`
/// are advisory offsets (spike 5 proved numeric offsets fragile across line-wraps).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextAnchor {
    pub selector: String,
    #[serde(default)]
    pub start: i64,
    #[serde(default)]
    pub end: i64,
    pub quote: String,
}

/// One item in a feedback batch (`plan-studio.md` / Feedback payload). A flat shape
/// carrying every item kind's fields so the wire contract evolves additively and the
/// UI's one-list model (`phase5-m3-ui.md` / Interpretation 5: a fired `data-action` is
/// an `action` item in the one item list) is preserved. `kind` discriminates:
/// `text_range | control | diagram_node | action | chat | element`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackItem {
    pub kind: String,
    /// `text_range`: the quote anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<TextAnchor>,
    /// The user's note (or null for a bare action).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `control`: the question key so a re-answer replaces the earlier one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_key: Option<String>,
    /// `control`: the captured value (string, bool, number, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    /// `diagram_node`: the mermaid diagram id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagram: Option<String>,
    /// `diagram_node`: the clicked node id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// `action`: the fired `data-action` name (e.g. `approve_plan`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// `text_range`: the anchor lifecycle status the chrome resolved
    /// (`anchored | drifted | re-anchored | unanchored`); an `unanchored` item still
    /// delivers `{ quote, body }` (`phase5-m3-ui.md` / Interpretation 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// One layout-audit finding (`plan-studio.md` / Layout audit gate). `kind` enum:
/// `horizontal_overflow | element_overflow | clipped_text | overlapping_text |
/// external_reference`; `external_reference` is always an error, `overlapping_text` is a
/// best-effort heuristic that never blocks alone (spike 5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutWarning {
    pub selector: String,
    pub kind: String,
    #[serde(default)]
    pub overflow_px: f64,
    #[serde(default)]
    pub viewport_width: f64,
    /// `error | warning`.
    pub severity: String,
}

/// A declared local service (`data-model.md` / services, `environments.md`). The wire
/// twin of the store row; values (ports declarations) travel as-is for the Settings UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub cmd: String,
    /// `per_worktree | shared` (shared is M4+).
    pub scope: String,
    /// Named port declarations for the port broker (e.g. `["HTTP","INSPECTOR"]`).
    #[serde(default)]
    pub ports: Vec<String>,
    /// A failed required service parks the card; an optional one does not.
    #[serde(default)]
    pub required: bool,
}

/// A gate run as carried by `gate.status`/`card.get` (`data-model.md` / gate_runs,
/// `gate.md` / Pipeline). The wire twin of the store row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateRunInfo {
    pub id: String,
    pub card_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// `checks | review | autofix | escalate | push | pr | ci | done`.
    pub step: String,
    /// `running | passed | failed | escalated`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_strictness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}

/// A gate finding as carried by `gate.status`/finding responses (`data-model.md` /
/// findings, `gate.md` / Findings). The wire twin of the store row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindingInfo {
    pub id: String,
    pub gate_run_id: String,
    pub card_id: String,
    /// `blocker | major | minor`.
    pub severity: String,
    /// `mechanical | intent`.
    pub category: String,
    /// `reviewer | check | ci`.
    pub source: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// `autofixed | accepted | fixed | skipped`; null while open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
}

/// A GitHub issue snapshot for the card workspace's Issue tab (`product.md` / Card
/// sources: GitHub issue import). Rendered from the card's `origin_data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubIssueInfo {
    pub number: u64,
    pub repo: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub url: String,
}

/// A Needs You queue item as carried by `fleet.status` (`data-model.md` /
/// needs_you_items). The wire twin of the store row; `resolved_*` fields are
/// null for open items (fleet snapshots carry open items only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeedsYouItem {
    pub id: String,
    pub card_id: String,
    pub kind: String,
    pub dedupe_key: String,
    pub score: i64,
    pub raised_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
}
