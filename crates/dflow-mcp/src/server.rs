//! The MCP tool surface: DapperFlow orchestration tools for the Concertmaster.
//!
//! Capability enforcement (`security.md` / Concertmaster capability scope): the
//! excluded set - merge, push, discard, permission changes, vault/env access,
//! recipe install - is not reachable from here because no tool for any of it
//! exists on this router; the exclusion is structural, not a permission check.
//! The server authenticates with the root token for now; the upgrade path is a
//! daemon-minted Concertmaster-scoped token (`security.md` / Token
//! architecture, per-task tokens generalized), at which point the same tool
//! surface keeps working and the daemon enforces what this server already
//! refuses to expose.
//!
//! Tool results are compact plain text with `[kind:id]` tokens (see `render`).

use std::collections::HashMap;
use std::sync::Arc;

use dflow_proto::{
    CardCreate, CardCreated, CardFilter, CardMove, CardQuery,
    CardQueryResult, CardResult, CardUpdate, DispatchStart, DispatchStarted, FleetStatus,
    FleetStatusResult, KnowFind, KnowFindResult, KnowGet, KnowGetResult, KnowIndex,
    KnowIndexResult, ProjectList, ProjectListResult, SessionAttach, SessionAttached,
    SessionDetach, SessionSummary, Simple,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::{Deserialize, Serialize};

use crate::daemon::{Daemon, DaemonError};
use crate::render;
use crate::runtime::DaemonEndpoint;
use crate::steer::{SteerGuard, STEERS_PER_HOUR};

/// Instructions shown to the mounting harness (the Concertmaster's contract).
const INSTRUCTIONS: &str = "\
DapperFlow orchestration tools: fleet oversight, board maintenance, dispatch, \
knowledge, and guarded steering, backed by the local DapperFlow daemon.
Conventions: every entity id appears as a bracketed token [kind:id] \
([card:...], [session:...], [project:...], [needs_you:...], [note:...]). \
Repeat these tokens verbatim whenever you mention an entity: the DapperFlow UI \
turns them into one-click deep links, and a mention without its token is a \
dead end for the user.
Capability scope: merge, push, discard, permission changes, env/vault access, \
and recipe install are human-only actions in the DapperFlow UI. This server \
has no tools for them by design; never claim you can perform them.
Steering: steer_session is a one-shot nudge for a stuck session, bounded to \
the stuck-recovery playbook, rate-limited per session, and refused absolutely \
on no_auto_steer adapters. Relay its attribution text so the user knows a \
Concertmaster steer happened.";

/// The DapperFlow MCP server. One instance serves one stdio transport.
#[derive(Clone)]
pub struct DflowMcp {
    tool_router: ToolRouter<DflowMcp>,
    steer: Arc<SteerGuard>,
    /// Explicit daemon endpoint override (tests); `None` = discover per call.
    endpoint: Option<DaemonEndpoint>,
}

// ---- tool parameter shapes (schemars doc comments become descriptions) ----

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BoardQueryParams {
    /// Filter to one project id (get ids from project_list); omit for all projects.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Filter to one board lane: inbox, shaping, ready, performing, verifying, needs_you, pr, done.
    #[serde(default)]
    pub lane: Option<String>,
    /// Filter to one card type: feature, bug, chore, test, investigation.
    #[serde(default)]
    pub card_type: Option<String>,
    /// Maximum number of cards to return.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CardCreateParams {
    /// Card title (short, imperative).
    pub title: String,
    /// Card type: feature, bug, chore, test, or investigation. Defaults to feature.
    #[serde(default)]
    pub card_type: Option<String>,
    /// Project id the card belongs to (from project_list). Omit for a cross-project card.
    #[serde(default)]
    pub project_id: Option<String>,
    /// The dispatch brief: what to do and how to know it is done.
    #[serde(default)]
    pub brief: Option<String>,
    /// Starting lane (defaults to inbox).
    #[serde(default)]
    pub lane: Option<String>,
    /// Starting priority (higher = more urgent; defaults to 0).
    #[serde(default)]
    pub priority: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CardUpdateParams {
    /// The card id to update (from board_query or card tokens).
    pub card_id: String,
    /// New title, if changing.
    #[serde(default)]
    pub title: Option<String>,
    /// New brief, if changing.
    #[serde(default)]
    pub brief: Option<String>,
    /// New card type, if changing.
    #[serde(default)]
    pub card_type: Option<String>,
    /// New priority, if changing.
    #[serde(default)]
    pub priority: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CardMoveParams {
    /// The card id to move.
    pub card_id: String,
    /// Target lane: inbox, shaping, ready, performing, verifying, needs_you, pr, done.
    /// The daemon arbitrates the move; moving an active card backward may be refused.
    pub column: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DispatchStartParams {
    /// The card to dispatch (it needs a brief; see card_update).
    pub card_id: String,
    /// Flow recipe name (presto, standard, deep, or an installed recipe). Omit for the project default.
    #[serde(default)]
    pub recipe: Option<String>,
    /// Configured agent launcher name (from the user's Settings > Agents). Preferred over harness.
    #[serde(default)]
    pub agent: Option<String>,
    /// Adapter family fallback (claude, codex, opencode, pi) when no launcher is named.
    #[serde(default)]
    pub harness: Option<String>,
    /// Model override for the harness, when supported.
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning-effort override for the harness, when supported.
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SessionPeekParams {
    /// The session id to peek at (from fleet_status).
    pub session_id: String,
    /// Maximum screen lines to return (default 40).
    #[serde(default)]
    pub lines: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KnowledgeDigestParams {
    /// Project id (from project_list). Required until the daemon infers a default.
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KnowledgeFindParams {
    /// Substring/tag query over note titles, tags, and descriptions.
    pub query: String,
    /// Restrict to one note type.
    #[serde(default)]
    pub note_type: Option<String>,
    /// Project id (from project_list).
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KnowledgeGetParams {
    /// The note id (from knowledge_find [note:...] tokens).
    pub id: String,
    /// Return the full body instead of the truncated default.
    #[serde(default)]
    pub full: Option<bool>,
    /// Project id (from project_list).
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SteerSessionParams {
    /// The stuck session to nudge (from fleet_status).
    pub session_id: String,
    /// The one-shot message to inject via verified submit. Keep it short and
    /// actionable (a continuation nudge), per the stuck-recovery playbook.
    pub text: String,
}

// ---- wire shapes protocol.md specifies but dflow-proto does not carry yet ----

/// `session.send_verified { session_id, text, submit }` (`protocol.md`).
#[derive(Debug, Serialize)]
struct SendVerifiedReq {
    session_id: String,
    text: String,
    submit: bool,
}

/// `session.send_verified` response `{ submitted, attempts }`.
#[derive(Debug, Deserialize)]
struct SendVerifiedRes {
    submitted: bool,
    #[serde(default)]
    attempts: u32,
}

/// Outcome of the steer flow, formatted after the wire work is done.
enum SteerOutcome {
    Refused(String),
    VerbMissing,
    Sent { submitted: bool, attempts: u32, remaining: usize, harness: String },
}

// ---- helpers ----

fn ok_text(text: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(text.into())]))
}

fn err_text(text: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![ContentBlock::text(text.into())]))
}

/// Render a daemon failure as a caller-visible tool error.
fn daemon_err(err: DaemonError) -> Result<CallToolResult, McpError> {
    err_text(err.to_string())
}

/// Fetch a card-id -> title map for joining Needs You items to card context.
fn card_titles(d: &mut Daemon) -> HashMap<String, String> {
    let res: Result<CardQueryResult, _> =
        d.request("card.query", CardQuery { filter: CardFilter::default() });
    match res {
        Ok(r) => r.cards.into_iter().map(|c| (c.id, c.title)).collect(),
        Err(_) => HashMap::new(), // titles are decoration; never fail the tool over them
    }
}

#[tool_router]
impl DflowMcp {
    /// A server that discovers the daemon per tool call (production path).
    pub fn new() -> DflowMcp {
        DflowMcp { tool_router: Self::tool_router(), steer: Arc::new(SteerGuard::new()), endpoint: None }
    }

    /// A server pinned to an explicit daemon endpoint (integration tests).
    pub fn with_endpoint(endpoint: DaemonEndpoint) -> DflowMcp {
        DflowMcp {
            tool_router: Self::tool_router(),
            steer: Arc::new(SteerGuard::new()),
            endpoint: Some(endpoint),
        }
    }

    /// The visible tool list (test surface for the capability allowlist).
    pub fn tools() -> Vec<rmcp::model::Tool> {
        Self::tool_router().list_all()
    }

    /// Run `f` against a fresh daemon connection on the blocking pool.
    async fn with_daemon<T, F>(&self, f: F) -> Result<T, DaemonError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Daemon) -> Result<T, DaemonError> + Send + 'static,
    {
        let endpoint = self.endpoint.clone();
        tokio::task::spawn_blocking(move || {
            let mut d = match endpoint {
                Some(ep) => Daemon::connect_to(&ep.endpoint, &ep.token)?,
                None => Daemon::connect()?,
            };
            f(&mut d)
        })
        .await
        .map_err(|e| DaemonError::Transport(format!("blocking task failed: {e}")))?
    }

    #[tool(
        description = "One fleet snapshot: every agent session (state, uptime, card, project, latest status note) plus the Needs You queue of items blocked on the human. Start here for 'what is going on'."
    )]
    pub async fn fleet_status(&self) -> Result<CallToolResult, McpError> {
        let res = self
            .with_daemon(|d| {
                let fleet: FleetStatusResult = d.request("fleet.status", FleetStatus {})?;
                let titles =
                    if fleet.needs_you.is_empty() { HashMap::new() } else { card_titles(d) };
                Ok((fleet, titles))
            })
            .await;
        match res {
            Ok((fleet, titles)) => ok_text(render::fleet(&fleet.sessions, &fleet.needs_you, &titles)),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "The Needs You queue alone: every item currently blocked on the human, highest score first, with card context. Use for 'what needs me right now?'."
    )]
    pub async fn needs_you_list(&self) -> Result<CallToolResult, McpError> {
        let res = self
            .with_daemon(|d| {
                let fleet: FleetStatusResult = d.request("fleet.status", FleetStatus {})?;
                let titles =
                    if fleet.needs_you.is_empty() { HashMap::new() } else { card_titles(d) };
                Ok((fleet, titles))
            })
            .await;
        match res {
            Ok((fleet, titles)) => ok_text(render::needs_you(&fleet.needs_you, &titles)),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(description = "List the registered projects with their ids, paths, delivery modes, and default branches.")]
    pub async fn project_list(&self) -> Result<CallToolResult, McpError> {
        let res = self
            .with_daemon(|d| d.request::<_, ProjectListResult>("project.list", ProjectList {}))
            .await;
        match res {
            Ok(r) => ok_text(render::projects(&r.projects)),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "Query board cards by project, lane, and/or type. Returns one compact line per card with its stable [card:id] token."
    )]
    pub async fn board_query(
        &self,
        Parameters(p): Parameters<BoardQueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = CardFilter {
            project_id: p.project_id,
            lane: p.lane,
            card_type: p.card_type,
            limit: p.limit,
        };
        let res = self
            .with_daemon(move |d| d.request::<_, CardQueryResult>("card.query", CardQuery { filter }))
            .await;
        match res {
            Ok(r) => ok_text(render::cards(&r.cards)),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "Create a board card. Give it a clear title; add a brief if it should ever be dispatchable. New cards land in Inbox unless a lane is given."
    )]
    pub async fn card_create(
        &self,
        Parameters(p): Parameters<CardCreateParams>,
    ) -> Result<CallToolResult, McpError> {
        if p.title.trim().is_empty() {
            return err_text("card_create needs a non-empty title");
        }
        let req = CardCreate {
            title: p.title,
            card_type: p.card_type.unwrap_or_else(|| "feature".into()),
            project_id: p.project_id,
            dial_recipe: None,
            brief: p.brief,
            priority: p.priority,
            lane: p.lane,
            fingerprint: None,
        };
        let res = self
            .with_daemon(move |d| d.request::<_, CardCreated>("card.create", req))
            .await;
        match res {
            Ok(r) => ok_text(format!("created {}", render::card_line(&r.card))),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(description = "Update a card's title, brief, type, or priority. Only the fields you pass change.")]
    pub async fn card_update(
        &self,
        Parameters(p): Parameters<CardUpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        let req = CardUpdate {
            card_id: p.card_id,
            title: p.title,
            card_type: p.card_type,
            dial_recipe: None,
            brief: p.brief,
            priority: p.priority,
        };
        let res = self
            .with_daemon(move |d| d.request::<_, CardResult>("card.update", req))
            .await;
        match res {
            Ok(r) => ok_text(format!("updated {}", render::card_line(&r.card))),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "Move a card to a board lane. The daemon arbitrates: it may refuse moves that fight live automation (e.g. dragging an actively performing card backward)."
    )]
    pub async fn card_move(
        &self,
        Parameters(p): Parameters<CardMoveParams>,
    ) -> Result<CallToolResult, McpError> {
        let req = CardMove { card_id: p.card_id, column: p.column };
        let res = self
            .with_daemon(move |d| d.request::<_, CardResult>("card.move", req))
            .await;
        match res {
            Ok(r) => ok_text(format!("moved {}", render::card_line(&r.card))),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "Dispatch a card to an agent: resolves the recipe, leases a worktree, composes the brief, and starts the session. The card needs a project and a brief first."
    )]
    pub async fn dispatch_start(
        &self,
        Parameters(p): Parameters<DispatchStartParams>,
    ) -> Result<CallToolResult, McpError> {
        let req = DispatchStart {
            card_id: p.card_id.clone(),
            recipe: p.recipe,
            agent: p.agent,
            harness: p.harness,
            model: p.model,
            effort: p.effort,
            budget_cards: None,
            budget_notes: None,
            audit: false,
            // The Concertmaster may never acknowledge in_place worktree strategy on the
            // user's behalf (security.md capability scope); an in_place recipe therefore
            // fails consent at the daemon, which is the correct outcome.
            ack_in_place: false,
        };
        let card_id = p.card_id;
        let res = self
            .with_daemon(move |d| d.request::<_, DispatchStarted>("dispatch.start", req))
            .await;
        match res {
            Ok(r) => {
                let agent = r.agent.as_deref().unwrap_or(&r.harness);
                ok_text(format!(
                    "dispatched [card:{card_id}] -> [session:{}] agent={agent} harness={} worktree={}",
                    r.session_id, r.harness, r.worktree_path
                ))
            }
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "Read a session's current screen as plain text (bounded lines). Use to check what a working or stuck agent is showing before steering or escalating. Side effect: it briefly attaches at 120x40, which can repaint the terminal for a human viewer."
    )]
    pub async fn session_peek(
        &self,
        Parameters(p): Parameters<SessionPeekParams>,
    ) -> Result<CallToolResult, McpError> {
        let lines = p.lines.unwrap_or(40).clamp(5, 200) as usize;
        let session_id = p.session_id.clone();
        let res = self
            .with_daemon(move |d| {
                let attach: SessionAttached = d.request(
                    "session.attach",
                    SessionAttach { session_id: p.session_id.clone(), cols: 120, rows: 40 },
                )?;
                let text = render::snapshot_text(&attach.snapshot, lines);
                let _: Simple =
                    d.request("session.detach", SessionDetach { session_id: p.session_id })?;
                Ok(text)
            })
            .await;
        match res {
            Ok(text) if text.trim().is_empty() => {
                ok_text(format!("[session:{session_id}] screen is currently blank"))
            }
            Ok(text) => ok_text(format!("[session:{session_id}] screen (last {lines} lines max):\n{text}")),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "A project's knowledge digest plus catalog counts: the standing context notes the project maintains about itself."
    )]
    pub async fn knowledge_digest(
        &self,
        Parameters(p): Parameters<KnowledgeDigestParams>,
    ) -> Result<CallToolResult, McpError> {
        let req = KnowIndex { project_id: p.project_id };
        let res = self
            .with_daemon(move |d| d.request::<_, KnowIndexResult>("know.index", req))
            .await;
        match res {
            Ok(r) => ok_text(render::know_index(&r)),
            Err(e) if e.is_unsupported() => err_text(knowledge_gap("know.index")),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(description = "Search a project's knowledge notes by substring/tag. Returns [note:id] tokens for knowledge_get.")]
    pub async fn knowledge_find(
        &self,
        Parameters(p): Parameters<KnowledgeFindParams>,
    ) -> Result<CallToolResult, McpError> {
        if p.query.trim().is_empty() {
            return err_text("knowledge_find needs a non-empty query");
        }
        let req = KnowFind { query: p.query, note_type: p.note_type, project_id: p.project_id };
        let res = self
            .with_daemon(move |d| d.request::<_, KnowFindResult>("know.find", req))
            .await;
        match res {
            Ok(r) => ok_text(render::know_find(&r)),
            Err(e) if e.is_unsupported() => err_text(knowledge_gap("know.find")),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(description = "Read one knowledge note by id (truncated by default; pass full=true for the whole body).")]
    pub async fn knowledge_get(
        &self,
        Parameters(p): Parameters<KnowledgeGetParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = p.id.clone();
        let req = KnowGet { id: p.id, full: p.full.unwrap_or(false), project_id: p.project_id };
        let res = self
            .with_daemon(move |d| d.request::<_, KnowGetResult>("know.get", req))
            .await;
        match res {
            Ok(r) => ok_text(render::know_get(&r, &id)),
            Err(e) if e.is_unsupported() => err_text(knowledge_gap("know.get")),
            Err(e) => daemon_err(e),
        }
    }

    #[tool(
        description = "One-shot guarded steer of a stuck session: injects a single message via verified submit. Bounded to the stuck-recovery playbook (nudge a session that stopped or drifted); never a dialogue - do not follow up by reading the reply. Refused absolutely on no_auto_steer adapters and rate-limited per session. Merge, push, discard, and approvals stay human even here."
    )]
    pub async fn steer_session(
        &self,
        Parameters(p): Parameters<SteerSessionParams>,
    ) -> Result<CallToolResult, McpError> {
        if p.text.trim().is_empty() {
            return err_text("steer_session needs non-empty text");
        }
        let steer = Arc::clone(&self.steer);
        let session_id = p.session_id.clone();
        let text = p.text.clone();
        let res = self
            .with_daemon(move |d| {
                // Locate the session and its adapter family first: `no_auto_steer`
                // must refuse before any budget spend or wire traffic.
                let list: dflow_proto::SessionListResult =
                    d.request("session.list", dflow_proto::SessionList {})?;
                let summary: Option<SessionSummary> =
                    list.sessions.into_iter().find(|s| s.session_id == p.session_id);
                let summary = match summary {
                    Some(s) => s,
                    None => {
                        return Ok(SteerOutcome::Refused(format!(
                            "steer refused: no session with id {} (check fleet_status)",
                            p.session_id
                        )))
                    }
                };
                if !summary.alive {
                    return Ok(SteerOutcome::Refused(format!(
                        "steer refused: [session:{}] has no live process (state={}); \
                         it cannot receive input. The human can resume it from the app.",
                        summary.session_id, summary.state
                    )));
                }
                if let Some(refusal) = crate::steer::no_steer_refusal(&summary.harness) {
                    return Ok(SteerOutcome::Refused(refusal.to_string()));
                }
                let remaining = match steer.try_acquire(&p.session_id) {
                    Ok(r) => r,
                    Err(refusal) => return Ok(SteerOutcome::Refused(refusal.to_string())),
                };
                let sent: Result<SendVerifiedRes, DaemonError> = d.request(
                    "session.send_verified",
                    SendVerifiedReq { session_id: p.session_id.clone(), text: p.text, submit: true },
                );
                match sent {
                    Ok(r) => Ok(SteerOutcome::Sent {
                        submitted: r.submitted,
                        attempts: r.attempts,
                        remaining,
                        harness: summary.harness,
                    }),
                    Err(e) if e.is_unsupported() => {
                        // Nothing reached the session; hand the budget slot back.
                        steer.refund(&p.session_id);
                        Ok(SteerOutcome::VerbMissing)
                    }
                    Err(e) => Err(e),
                }
            })
            .await;
        match res {
            Ok(SteerOutcome::Refused(msg)) => ok_text(msg),
            Ok(SteerOutcome::VerbMissing) => err_text(
                "steer not delivered: this daemon build does not route session.send_verified \
                 yet (it is specced in protocol.md and lands with the M4 daemon work). \
                 Escalate to the human instead.",
            ),
            Ok(SteerOutcome::Sent { submitted: true, attempts, remaining, harness }) => ok_text(format!(
                "steer submitted to [session:{session_id}] (harness={harness}, attempts={attempts}). \
                 One-shot: do not read the session's reply or follow up. \
                 Attribution to relay: a Concertmaster steer was injected with the text: \"{text}\". \
                 Steer budget left for this session this hour: {remaining} of {STEERS_PER_HOUR}."
            )),
            Ok(SteerOutcome::Sent { submitted: false, attempts, .. }) => err_text(format!(
                "steer NOT submitted after {attempts} verified-submit attempts on \
                 [session:{session_id}]; the composer never accepted the text. Do not retry; \
                 escalate to the human (this is the failed-submit escalation rule)."
            )),
            Err(e) => daemon_err(e),
        }
    }
}

impl Default for DflowMcp {
    fn default() -> Self {
        Self::new()
    }
}

/// The shared message for knowledge verbs the daemon does not route yet.
fn knowledge_gap(verb: &str) -> String {
    format!(
        "{verb} is not routed for desktop-scope connections in this daemon build \
         (today the daemon serves know.* to per-task agent tokens only; routing it \
         for the Concertmaster scope is queued daemon work). The project's knowledge \
         is still browsable in the DapperFlow app."
    )
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DflowMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("dflow-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The complete, closed tool allowlist. Adding a tool must be a conscious,
    /// reviewed act: the Concertmaster capability scope (`security.md`) is
    /// enforced by this list being exactly the safe surface.
    const ALLOWED: [&str; 13] = [
        "board_query",
        "card_create",
        "card_move",
        "card_update",
        "dispatch_start",
        "fleet_status",
        "knowledge_digest",
        "knowledge_find",
        "knowledge_get",
        "needs_you_list",
        "project_list",
        "session_peek",
        "steer_session",
    ];

    #[test]
    fn tool_surface_is_exactly_the_allowlist() {
        let mut names: Vec<String> =
            DflowMcp::tools().iter().map(|t| t.name.to_string()).collect();
        names.sort();
        assert_eq!(names, ALLOWED.to_vec());
    }

    #[test]
    fn excluded_capabilities_have_no_tool() {
        // The excluded set (`security.md` / Concertmaster capability scope) plus
        // the daemon verbs that could reach it. No tool name may even hint at them.
        let forbidden = [
            "merge", "push", "discard", "permission", "vault", "env", "secret",
            "recipe", "install", "kill", "shutdown", "signal", "cancel", "remove",
            "token", "auth", "agents",
        ];
        for tool in DflowMcp::tools() {
            let name = tool.name.to_string();
            for bad in forbidden {
                assert!(
                    !name.contains(bad),
                    "tool '{name}' matches excluded capability '{bad}'"
                );
            }
        }
    }

    #[test]
    fn every_tool_has_description_and_serializable_schema() {
        for tool in DflowMcp::tools() {
            let desc = tool.description.as_deref().unwrap_or("");
            assert!(desc.len() > 20, "tool {} needs an LLM-facing description", tool.name);
            let schema = serde_json::to_string(tool.input_schema.as_ref())
                .unwrap_or_else(|e| panic!("schema for {} does not serialize: {e}", tool.name));
            assert!(schema.contains("\"type\""), "schema for {} looks empty: {schema}", tool.name);
        }
    }

    #[test]
    fn param_schemas_carry_field_docs() {
        let tools = DflowMcp::tools();
        let steer = tools.iter().find(|t| t.name == "steer_session").unwrap();
        let schema = serde_json::to_value(steer.input_schema.as_ref()).unwrap();
        let props = schema["properties"].as_object().expect("object schema");
        assert!(props.contains_key("session_id") && props.contains_key("text"));
        let create = tools.iter().find(|t| t.name == "card_create").unwrap();
        let schema = serde_json::to_value(create.input_schema.as_ref()).unwrap();
        assert!(schema["required"].as_array().unwrap().iter().any(|v| v == "title"));
    }

    #[test]
    fn instructions_state_the_link_token_and_scope_contract() {
        let info = DflowMcp::new().get_info();
        let instructions = info.instructions.expect("instructions set");
        assert!(instructions.contains("[kind:id]") || instructions.contains("[card:"));
        assert!(instructions.contains("human-only"));
    }
}
