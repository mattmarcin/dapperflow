//! Request handlers for the Phase 1 protocol families (`protocol.md`):
//! `project.*`, `card.*`, `dispatch.*`, `session.rename`, and the enriched
//! `session.list`. Pure request -> response functions over `AppState`; the
//! connection layer (`conn.rs`) does envelope framing and error replies.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dflow_core::recipe::Stage;
use dflow_core::{
    agent_source, agents, bundled_manifests, default_command, event_kind, harness, knowledge,
    lease_state, service_scope, session_state, setting_key, steer, AgentPatch, CardPatch,
    CardQueryFilter, NewAgent, NewCard, Recipe, Session, SessionRow, SessionSpec, ServiceStart,
    Store, StoreError, SubmitConfig, WorktreeError, WorktreeStrategy,
};
use dflow_proto::{
    AgentAdd, AgentContextResult, AgentRemove, AgentRemoved, AgentResult, AgentUpdate,
    AgentsDetected, AgentsListResult, ArtifactGet, ArtifactGetResult, ArtifactRegister,
    ArtifactRegistered, CardCreate, CardCreated, CardGet, CardGetResult, CardMove, CardQuery,
    CardQueryResult, CardResult, CardUpdate, ConcertmasterMinted, DetectedCli, DispatchCancel,
    DispatchCancelled, DispatchStart, DispatchStarted, EnvCleaned, EnvCleanup, EnvDelete, EnvDeleted,
    EnvEntryInfo, EnvImport, EnvImportResult, EnvList, EnvListResult, EnvMaterialize, EnvMaterialized,
    EnvSet, EnvSetResult, FeedbackItem, FeedbackPoll, FeedbackPollResult, FeedbackSubmit,
    FeedbackSubmitResult, FleetStatusResult, KnowAdd, KnowAddResult, KnowCatalogGroup, KnowFind,
    KnowFindResult, KnowGet, KnowGetResult, KnowIndex, KnowIndexResult, KnowNote, KnowNoteHit,
    LanEnable, LanPair, LanPairing, LanRevoke, LanRevoked, LanState, NeedsYouItem, NeedsYouListResult,
    NeedsYouResolve, NeedsYouResolved, NotifyForward, PairingPayload, PhonePairing, ProjectAdd,
    ProjectAdded, ProjectList, ProjectListResult, ProjectUpdate, ProjectUpdated, ProtocolError,
    RoundDigest, RoundDigestResult, RoundStart, RoundStarted, SelfReport, SelfReportResult,
    ServiceAdd, ServiceList, ServiceListResult, ServiceRemove, ServiceRemoved, ServiceResult,
    SendVerifiedResult, SessionListResult, SessionPeek, SessionPeeked, SessionRename, SessionResume,
    SessionResumed, SessionSummary, SetNote, Simple,
};
use ulid::Ulid;

use crate::server::AppState;
use crate::tokens::{AgentToken, RoundToken, TokenScope};

/// Body line cap for `know get` before truncation (the AXI `--full` escape hatch).
const KNOW_GET_LINE_CAP: usize = 40;
/// Max hits `know find` returns before pointing at a narrower query.
const KNOW_FIND_LIMIT: usize = 25;

/// Default page size for `card.get` events when the client sends no limit.
const DEFAULT_EVENTS_LIMIT: i64 = 50;
/// Terminal size dispatch sessions start with until a client attaches and resizes.
const DISPATCH_COLS: u16 = 120;
const DISPATCH_ROWS: u16 = 32;
/// Max characters kept in the `first_prompt` preview.
const FIRST_PROMPT_PREVIEW: usize = 120;

// ---- project.* ----

/// `project.add { path }`: validate a git repo root, detect the default branch,
/// store it. Re-adding an already-registered root returns the existing project.
pub fn project_add(state: &AppState, req: ProjectAdd) -> Result<ProjectAdded, ProtocolError> {
    let input = PathBuf::from(&req.path);
    if !input.is_dir() {
        return Err(ProtocolError::bad_request(format!(
            "path is not a directory: {}",
            input.display()
        )));
    }
    let toplevel = git_capture(&input, &["rev-parse", "--show-toplevel"])
        .map_err(|e| ProtocolError::bad_request(format!("not a git repository: {e}")))?;
    let toplevel = PathBuf::from(toplevel.trim());
    let same_root = match (std::fs::canonicalize(&input), std::fs::canonicalize(&toplevel)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same_root {
        return Err(ProtocolError::bad_request(format!(
            "path is inside a git repo but is not its root (root is {})",
            toplevel.display()
        )));
    }
    let stored_path = toplevel.to_string_lossy().into_owned();

    // Idempotent add: an already-registered root returns the existing row.
    if let Some(existing) = state.store.get_project_by_path(&stored_path).map_err(store_err)? {
        return Ok(ProjectAdded { project_id: existing.id.clone(), project: existing });
    }

    let default_branch = detect_default_branch(&toplevel);
    let name = toplevel
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());

    let project = state
        .store
        .add_project(&stored_path, &name, &default_branch, "pr")
        .map_err(store_err)?;
    Ok(ProjectAdded { project_id: project.id.clone(), project })
}

/// `project.update { project_id, mode?, check_cmds?, default_recipe? }`.
pub fn project_update(state: &AppState, req: ProjectUpdate) -> Result<ProjectUpdated, ProtocolError> {
    if let Some(mode) = req.mode.as_deref() {
        if mode != "pr" && mode != "local_only" {
            return Err(ProtocolError::bad_request(format!(
                "mode must be 'pr' or 'local_only', got '{mode}'"
            )));
        }
    }
    // Round schedules (`product.md` / Concertmaster rounds): Some("") clears, Some(json)
    // sets, None leaves unchanged. Set before the field read-back so the row is fresh.
    if let Some(schedule) = req.rounds_schedule.as_deref() {
        state.store.set_project_schedule(&req.project_id, false, Some(schedule)).map_err(store_err)?;
    }
    if let Some(schedule) = req.gardener_schedule.as_deref() {
        state.store.set_project_schedule(&req.project_id, true, Some(schedule)).map_err(store_err)?;
    }
    let project = state
        .store
        .update_project(
            &req.project_id,
            req.mode.as_deref(),
            req.check_cmds.as_deref(),
            req.default_recipe.as_deref(),
        )
        .map_err(store_err)?;
    Ok(ProjectUpdated { project })
}

/// `project.list {}`.
pub fn project_list(state: &AppState, _req: ProjectList) -> Result<ProjectListResult, ProtocolError> {
    let projects = state.store.list_projects().map_err(store_err)?;
    Ok(ProjectListResult { projects })
}

// ---- card.* ----

/// `card.create { title, type, project_id?, dial_recipe?, brief? }`.
pub fn card_create(state: &AppState, req: CardCreate) -> Result<CardCreated, ProtocolError> {
    if req.title.trim().is_empty() {
        return Err(ProtocolError::bad_request("card title must not be empty"));
    }
    if let Some(pid) = &req.project_id {
        if state.store.get_project(pid).map_err(store_err)?.is_none() {
            return Err(ProtocolError::not_found(format!("project {pid}")));
        }
    }
    let card = state
        .store
        .create_card(NewCard {
            project_id: req.project_id,
            card_type: req.card_type,
            title: req.title,
            lane: req.lane.unwrap_or_else(|| "inbox".to_string()),
            dial_recipe: req.dial_recipe,
            brief: req.brief,
            priority: req.priority.unwrap_or(0),
            ..Default::default()
        })
        .map_err(store_err)?;
    Ok(CardCreated { card_id: card.id.clone(), card, dedupe: None })
}

/// `card.update { card_id, ... }`.
pub fn card_update(state: &AppState, req: CardUpdate) -> Result<CardResult, ProtocolError> {
    let card = state
        .store
        .update_card(
            &req.card_id,
            CardPatch {
                title: req.title,
                card_type: req.card_type,
                dial_recipe: req.dial_recipe,
                brief: req.brief,
                priority: req.priority,
            },
        )
        .map_err(store_err)?;
    Ok(CardResult { card })
}

/// `card.move { card_id, column }` (wire `column` maps to the DB `lane`).
pub fn card_move(state: &AppState, req: CardMove) -> Result<CardResult, ProtocolError> {
    if req.column.trim().is_empty() {
        return Err(ProtocolError::bad_request("column must not be empty"));
    }
    let card = state.store.move_card(&req.card_id, &req.column).map_err(store_err)?;
    Ok(CardResult { card })
}

/// `card.query { filter }`.
pub fn card_query(state: &AppState, req: CardQuery) -> Result<CardQueryResult, ProtocolError> {
    let cards = state
        .store
        .query_cards(&CardQueryFilter {
            project_id: req.filter.project_id,
            lane: req.filter.lane,
            card_type: req.filter.card_type,
            limit: req.filter.limit,
        })
        .map_err(store_err)?;
    Ok(CardQueryResult { cards })
}

/// `card.get { card_id }`: card + sessions + latest events (paged).
pub fn card_get(state: &AppState, req: CardGet) -> Result<CardGetResult, ProtocolError> {
    let card = state
        .store
        .get_card(&req.card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("card {}", req.card_id)))?;
    let sessions = state
        .store
        .card_session_rows(&req.card_id)
        .map_err(store_err)?
        .into_iter()
        .map(|row| summarize(state, row))
        .collect();
    let events = state
        .store
        .card_events(
            &req.card_id,
            req.events_before.as_deref(),
            req.events_limit.unwrap_or(DEFAULT_EVENTS_LIMIT).clamp(1, 500),
        )
        .map_err(store_err)?;
    let artifacts = state
        .store
        .card_artifacts(&req.card_id)
        .map_err(store_err)?
        .iter()
        .map(|a| a.to_meta())
        .collect();
    Ok(CardGetResult { card, sessions, events, artifacts })
}

// ---- dispatch.* ----

/// The launch resolved for a dispatch: the argv, the env to merge at spawn, the
/// adapter family recorded on the session row, and the launcher name/id when one was
/// used (`adapters.md` / dispatch resolves launcher first, adapter behavior second).
struct ResolvedLaunch {
    /// Adapter family recorded on `sessions.harness` (behavior key).
    harness: String,
    command: Vec<String>,
    /// Extra env merged over the base spawn env, launcher wins.
    extra_env: BTreeMap<String, String>,
    /// The launcher name, for the response and the `dispatched` event (null: built-in).
    agent_name: Option<String>,
    /// The launcher id recorded on `sessions.agent_id` (null: built-in).
    agent_id: Option<String>,
}

/// Resolve a dispatch to a concrete launch (`adapters.md` / dispatch flow step 1).
///
/// Precedence: explicit `agent` (launcher name or id, must be enabled) > `harness_name`
/// (a same-named enabled launcher if one exists, else the legacy built-in table). The
/// caller pre-merges the axes (explicit dispatch params win over the recipe's axes,
/// `recipes.md` / Engine integration), so `harness_name`/`model`/`effort` arrive here
/// already effective; an absent harness falls back to the built-in default.
fn resolve_launch(
    state: &AppState,
    agent_ref: Option<&str>,
    harness_name: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    brief: &str,
) -> Result<ResolvedLaunch, ProtocolError> {
    if let Some(agent_ref) = agent_ref.map(str::trim).filter(|s| !s.is_empty()) {
        let agent = state
            .store
            .resolve_agent(agent_ref)
            .map_err(store_err)?
            .ok_or_else(|| ProtocolError::not_found(format!("agent '{agent_ref}'")))?;
        if !agent.enabled {
            return Err(ProtocolError::bad_request(format!(
                "launcher '{}' is disabled; enable it before dispatch",
                agent.name
            )));
        }
        return Ok(launch_from_agent(agent, brief, model, effort));
    }

    let name = harness_name
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(harness::DEFAULT_HARNESS)
        .to_string();
    match state.store.get_agent_by_name(&name).map_err(store_err)? {
        Some(agent) if agent.enabled => Ok(launch_from_agent(agent, brief, model, effort)),
        _ => {
            let command = harness::harness_command(&name, brief, model, effort).ok_or_else(|| {
                ProtocolError::bad_request(format!(
                    "unknown harness '{name}' (built-ins: claude, codex, opencode; or add a launcher)"
                ))
            })?;
            Ok(ResolvedLaunch {
                harness: name,
                command,
                extra_env: BTreeMap::new(),
                agent_name: None,
                agent_id: None,
            })
        }
    }
}

/// Build a launch from a resolved launcher: manifest launch line + command + model/
/// effort axes + extra args, with the brief and the launcher's extra env
/// (`product.md` / Settings > Agents).
fn launch_from_agent(
    agent: dflow_proto::Agent,
    brief: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> ResolvedLaunch {
    let command = harness::launcher_command(
        &agent.name,
        &agent.adapter,
        &agent.command,
        brief,
        &agent.extra_args,
        model,
        effort,
    );
    ResolvedLaunch {
        harness: agent.adapter,
        command,
        extra_env: agent.extra_env,
        agent_name: Some(agent.name),
        agent_id: Some(agent.id),
    }
}

/// `dispatch.start { card_id, recipe?, agent?, harness?, model?, effort? }`: resolve
/// the recipe FIRST, then the launcher; lease a worktree per the recipe's strategy and
/// launch in it (`adapters.md` / dispatch flow; `recipes.md` / Engine integration:
/// everything downstream reads recipe output, not hardcoded policy).
pub fn dispatch_start(state: &AppState, req: DispatchStart) -> Result<DispatchStarted, ProtocolError> {
    let card = state
        .store
        .get_card(&req.card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("card {}", req.card_id)))?;
    let project_id = card.project_id.clone().ok_or_else(|| {
        ProtocolError::bad_request("card has no project; dispatch needs a project worktree")
    })?;
    let project = state
        .store
        .get_project(&project_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("project {project_id}")))?;

    // Step 1: resolve the recipe (param > card dial > project default > "standard").
    // A privileged recipe without a valid grant fails HERE with a consent error,
    // before any lease or launch (`security.md` / Recipe trust tiers).
    let resolved = crate::recipes::resolve_dispatch_recipe(state, req.recipe.as_deref(), &card, &project)?;
    let recipe = Arc::new(resolved.recipe.clone());

    // Axes: explicit dispatch params win over the recipe's axes; an explicit launcher
    // choice also overrides the recipe's harness axis (`recipes.md`: "explicit dispatch
    // params still win").
    let agent_param = req.agent.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let harness_param = req
        .harness
        .clone()
        .filter(|h| !h.trim().is_empty())
        .or_else(|| {
            if agent_param.is_none() {
                recipe.harness_axis().map(str::to_string)
            } else {
                None
            }
        });
    let model = req.model.clone().or_else(|| recipe.model_axis().map(str::to_string));
    let effort = req.effort.clone().or_else(|| recipe.effort_axis().map(str::to_string));

    let brief = compose_dispatch_brief(state, &card, &project, &recipe);
    let launch = resolve_launch(
        state,
        agent_param,
        harness_param.as_deref(),
        model.as_deref(),
        effort.as_deref(),
        &brief,
    )?;

    // Recipe x harness compatibility: an MCP-mounting recipe on a harness without
    // verified MCP support fails at dispatch, not mid-run (`recipes.md`). The mounts
    // themselves are recorded in the dispatched event as meta; mounting them into
    // harness launch args is deferred to the Concertmaster phase (M4).
    crate::recipes::validate_mcp_for_harness(&recipe, &launch.harness)?;

    // Worktree strategy (`recipes.md` / implement.worktree). `in_place` requires the
    // privileged grant (already enforced by recipe resolution) AND a per-dispatch ack.
    let strategy = recipe.worktree_strategy();
    if strategy == WorktreeStrategy::InPlace && !req.ack_in_place {
        return Err(ProtocolError::bad_request(format!(
            "recipe '{}' works in place (edits the project checkout directly); dispatch again with \
             ack_in_place: true to confirm",
            recipe.name
        )));
    }

    // Budgets: the recipe supplies them; explicit dispatch params override
    // (`recipes.md` / budgets; the bare parameter from the dflow-CLI phase remains an
    // override seam).
    let budget_cards = req.budget_cards.or(recipe.budgets.and_then(|b| b.cards));
    let budget_notes = req.budget_notes.or(recipe.budgets.and_then(|b| b.notes));

    // The dispatched event records the recipe name and version so timelines show which
    // flow produced which outcome (`recipes.md` / Engine integration), plus the
    // resolved axes, strategy, budgets, and declared MCP mounts (meta only in M2).
    state
        .store
        .append_card_event(
            &req.card_id,
            dflow_core::event_kind::DISPATCHED,
            serde_json::json!({
                "harness": launch.harness,
                "agent": launch.agent_name,
                "model": model,
                "effort": effort,
                "recipe": recipe.name,
                "recipe_version": recipe.version,
                "recipe_scope": resolved.scope.as_str(),
                "recipe_hash": resolved.hash,
                "worktree_strategy": strategy.as_str(),
                "budgets": { "cards": budget_cards, "notes": budget_notes },
                "mcp": recipe.mcp.iter().map(|m| serde_json::json!({
                    "name": m.name,
                    "command": m.command,
                    "stages": m.stages.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    "mounted": false,
                })).collect::<Vec<_>>(),
            }),
        )
        .map_err(store_err)?;

    // Lease per strategy: pooled reuses a warm slot, fresh always creates a new one,
    // in_place runs in the project checkout with no lease at all.
    let (worktree_row, cwd) = match strategy {
        WorktreeStrategy::Pooled => {
            let wt = state.worktrees.lease(&project, &req.card_id).map_err(wt_err)?;
            let cwd = PathBuf::from(&wt.path);
            (Some(wt), cwd)
        }
        WorktreeStrategy::Fresh => {
            let wt = state.worktrees.lease_fresh(&project, &req.card_id).map_err(wt_err)?;
            let cwd = PathBuf::from(&wt.path);
            (Some(wt), cwd)
        }
        WorktreeStrategy::InPlace => (None, PathBuf::from(&project.path)),
    };

    // Materialize the project env vault into the session (`environments.md` / Worktree
    // materialization lifecycle). A leased (pooled/fresh) worktree gets `var`/`secret`
    // entries injected into the spawn env AND `file` entries written to their target
    // paths; an `in_place` dispatch gets env-only, because writing then shredding files
    // in the user's real checkout would destroy the user's own files.
    let materialized = if worktree_row.is_some() {
        state.env_vault.materialize(&state.store, &project_id, &cwd).map_err(env_err)?
    } else {
        state.env_vault.materialize_env_only(&state.store, &project_id).map_err(env_err)?
    };
    // Evidence on the timeline: counts and the file target names, never any value
    // (`data-model.md` / env_materialized; the scrubber also guards this payload).
    state
        .store
        .append_card_event(
            &req.card_id,
            dflow_core::event_kind::ENV_MATERIALIZED,
            serde_json::json!({
                "vars": materialized.vars,
                "secrets": materialized.secrets,
                "files": materialized.file_targets,
                "in_place": worktree_row.is_none(),
            }),
        )
        .map_err(store_err)?;

    // Start declared per-worktree services with the port broker (`environments.md` /
    // Local services and the port broker; `adapters.md` dispatch flow step 4). A required
    // service that fails its process-alive health check parks the card (`service_failed`)
    // and aborts the dispatch before the agent launches, so no agent flails against a
    // dead backend. The allocated `DFLOW_PORT_<NAME>` env is injected into the agent's
    // spawn env. Only a leased worktree gets services (an in-place dispatch is skipped).
    let mut service_port_env: BTreeMap<String, String> = BTreeMap::new();
    if let Some(wt) = &worktree_row {
        let per_worktree: Vec<_> = state
            .store
            .list_services(&project_id)
            .map_err(store_err)?
            .into_iter()
            .filter(|s| s.scope == service_scope::PER_WORKTREE)
            .collect();
        if !per_worktree.is_empty() {
            let started = state.services.start_worktree(&wt.id, &per_worktree, &cwd, &materialized.env);
            for outcome in &started.outcomes {
                match outcome {
                    ServiceStart::Started { name, ports, pid, .. } => {
                        let _ = state.store.append_card_event(
                            &req.card_id,
                            dflow_core::event_kind::SERVICE_STARTED,
                            serde_json::json!({ "service": name, "ports": ports, "pid": pid }),
                        );
                    }
                    ServiceStart::Failed { name, reason, required } => {
                        let _ = state.store.append_card_event(
                            &req.card_id,
                            dflow_core::event_kind::SERVICE_FAILED,
                            serde_json::json!({ "service": name, "reason": reason, "required": required }),
                        );
                    }
                }
            }
            if started.has_required_failure() {
                // Stop everything we started, park the card, and abort before launching.
                state.services.stop_worktree(&wt.id);
                let (name, reason) = started.required_failure().unwrap_or(("service", "failed"));
                let score = needs_input_score(state, Some(&req.card_id));
                let _ = state.store.raise_needs_you(
                    &req.card_id,
                    "service_failed",
                    &format!("service_failed:{}:{}", wt.id, name),
                    score,
                );
                return Err(ProtocolError::internal(format!(
                    "required service '{name}' failed to start ({reason}); the card is parked in \
                     Needs You (service_failed) and the agent was not launched"
                )));
            }
            service_port_env = started.port_env;
        }
    }

    // Evidence that the recipe's guidance reached the brief (`data-model.md` event
    // taxonomy / brief_composed): which stage sections were injected and how large the
    // composed brief is, without dumping the whole text into the event log.
    let guidance_stages: Vec<&str> = recipe
        .guidance
        .iter()
        .filter(|g| recipe.has_stage(g.stage))
        .map(|g| g.stage.as_str())
        .collect();
    state
        .store
        .append_card_event(
            &req.card_id,
            dflow_core::event_kind::BRIEF_COMPOSED,
            serde_json::json!({
                "recipe": recipe.name,
                "recipe_version": recipe.version,
                "guidance_stages": guidance_stages,
                "chars": brief.chars().count(),
            }),
        )
        .map_err(store_err)?;

    let card_ulid = Ulid::from_string(&req.card_id)
        .map_err(|_| ProtocolError::bad_request("card_id is not a ULID"))?;
    let worktree_ulid = match &worktree_row {
        Some(wt) => Some(
            Ulid::from_string(&wt.id)
                .map_err(|_| ProtocolError::internal("worktree id is not a ULID"))?,
        ),
        None => None,
    };

    // Spawn env: the materialized vault (`var`/`secret`) as the base, then the port
    // broker's `DFLOW_PORT_<NAME>` vars (so the agent sees each service's live URL), then
    // the launcher's extra env, which wins on conflict (`product.md`, `environments.md`).
    let mut env = BTreeMap::new();
    env.extend(materialized.env.clone());
    env.extend(service_port_env);
    env.extend(launch.extra_env);

    // Mint the per-task token and inject the agent-CLI env (DFLOW_TOKEN/DFLOW_CARD/
    // DFLOW_ENDPOINT + `dflow` on PATH) before spawn: env can only enter a process at
    // spawn (`adapters.md` dispatch flow steps 5-6; `security.md` / Per-task tokens).
    // The token carries the resolved recipe so stage arbitration and budget
    // enforcement read recipe policy, not request flags.
    let (task_token, token_handle) = state.tokens.mint(TokenScope {
        card_id: Some(req.card_id.clone()),
        project_id: Some(project_id.clone()),
        audit: req.audit,
        budget_cards,
        budget_notes,
        recipe: Some(Arc::clone(&recipe)),
        gate_run_id: None,
    });
    inject_agent_env(state, &mut env, &task_token, Some(&req.card_id));

    // Tier-2 native signals: a claude-family launch gets a session-scoped --settings
    // file wiring lifecycle hooks to the daemon (never touching user/project settings);
    // a codex-family launch gets `-c notify` pointed at the `dflow notify-forward`
    // bridge, which forwards agent-turn-complete over the per-task token.
    let (command, hook_token) = crate::hooks::wire_native_hooks(state, &launch.harness, launch.command);
    let command = crate::hooks::wire_codex_notify(&launch.harness, command);

    let spec = SessionSpec {
        harness: launch.harness.clone(),
        command,
        cols: DISPATCH_COLS,
        rows: DISPATCH_ROWS,
        cwd: Some(cwd.clone()),
        env,
        card_id: Some(card_ulid),
        project_id: Some(project_id.clone()),
        worktree_id: worktree_ulid,
        agent_id: launch.agent_id,
        model,
        effort,
        first_prompt: Some(harness::preview(&brief, FIRST_PROMPT_PREVIEW)),
        title: None,
        resumed_from: None,
        scrollback_dir: Some(state.data_dir.scrollback_dir()),
    };
    let session = state.sessions.create(spec).map_err(|e| {
        // The lease stays with the card so a retry reuses it; the failure is loud.
        ProtocolError::internal(format!("could not launch harness '{}': {e}", launch.harness))
    })?;
    // Bind the hook token to this session so incoming hook POSTs resolve to it.
    if let Some(token) = hook_token {
        state.hooks.register(token, session.id.to_string());
    }
    // Bind the per-task token to the session so the agent CLI's verbs resolve to it.
    token_handle.bind_session(&session.id.to_string());
    // Register this session's materialized secret values with the scrubber, so any
    // durable capture that leaves the session (peeks, event payloads) redacts them
    // (`security.md` / Secret handling policy). Keyed by session id; dropped when the
    // session ends (server supervisor) and the files shredded at worktree return.
    if !materialized.secret_values.is_empty() {
        dflow_core::secret::registry().register(&session.id.to_string(), materialized.secret_values);
    }
    // Watch for and answer a trust dialog per the manifest (dispatch flow step 7).
    spawn_trust_watcher(Arc::clone(&session), launch.harness.clone());

    let (worktree_id, worktree_path) = match worktree_row {
        Some(wt) => (wt.id, wt.path),
        // in_place: no lease; the "worktree" is the project checkout itself.
        None => (String::new(), project.path.clone()),
    };
    Ok(DispatchStarted {
        session_id: session.id.to_string(),
        worktree_id,
        worktree_path,
        harness: launch.harness,
        agent: launch.agent_name,
        recipe: Some(recipe.name.clone()),
        recipe_version: Some(recipe.version),
    })
}

/// `dispatch.cancel { card_id }`: kill the card's live sessions and return (or
/// park) its leased worktrees.
pub fn dispatch_cancel(state: &AppState, req: DispatchCancel) -> Result<DispatchCancelled, ProtocolError> {
    let card = state
        .store
        .get_card(&req.card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("card {}", req.card_id)))?;
    let card_ulid = Ulid::from_string(&req.card_id)
        .map_err(|_| ProtocolError::bad_request("card_id is not a ULID"))?;

    // Kill live sessions first so nothing writes into the worktree during return, and
    // drop each one's materialized secrets from the scrubber registry.
    let live = state.sessions.sessions_for_card(&card_ulid);
    let mut cancelled = 0usize;
    for session in live {
        dflow_core::secret::registry().unregister(&session.id.to_string());
        if state.sessions.kill(&session.id) {
            cancelled += 1;
        }
    }

    // Return every worktree this card holds.
    let mut worktree_state: Option<String> = None;
    if let Some(project_id) = &card.project_id {
        let held: Vec<_> = state
            .store
            .worktrees_for_project(project_id)
            .map_err(store_err)?
            .into_iter()
            .filter(|w| {
                w.leased_by_card.as_deref() == Some(req.card_id.as_str())
                    && w.lease_state == lease_state::LEASED
            })
            .collect();
        for wt in held {
            // Stop this worktree's per-worktree services (their trees die with the Job
            // Object) before touching its files (`environments.md`).
            let stopped = state.services.stop_worktree(&wt.id);
            if stopped > 0 {
                tracing::info!(worktree = %wt.id, stopped, "stopped worktree services at return");
            }

            // Drift guard: diff each materialized env file against the vault BEFORE
            // shredding, and raise a value-masked `env_drift` Needs You for each change so
            // environment knowledge accretes instead of evaporating (`environments.md` /
            // Drift guard). The raise side is ours; absorbing is a later `env.set`.
            raise_env_drift(state, &req.card_id, project_id, &wt.id, &PathBuf::from(&wt.path));

            // Shred materialized secret files BEFORE the worktree re-enters the pool
            // (`environments.md` / On return: shred materialized secret files). `git
            // clean` in `release` only unlinks; the vault overwrites-then-deletes. This
            // must precede release regardless of clean/dirty outcome, since a dirty
            // worktree is not cleaned at all. Best-effort: a shred failure is logged, not
            // fatal to the return.
            match state.env_vault.cleanup(&state.store, project_id, &PathBuf::from(&wt.path)) {
                Ok(n) if n > 0 => tracing::info!(worktree = %wt.id, shredded = n, "shredded materialized secret files"),
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, worktree = %wt.id, "vault cleanup failed at return"),
            }
            state.worktrees.release(&wt.id).map_err(wt_err)?;
            let after = state
                .store
                .get_worktree(&wt.id)
                .map_err(store_err)?
                .map(|w| w.lease_state)
                .unwrap_or_else(|| lease_state::RETIRED.to_string());
            worktree_state = Some(after);
        }
    }

    Ok(DispatchCancelled { cancelled, worktree_state })
}

/// Diff a returning worktree's materialized env files against the vault and raise a
/// value-masked `env_drift` Needs You item (plus an `env_drift` event) for each drifted
/// file (`environments.md` / Drift guard; `security.md`: diffs shown with values masked).
/// Runs BEFORE the files are shredded. Best-effort: a diff failure is logged, not fatal.
fn raise_env_drift(state: &AppState, card_id: &str, project_id: &str, worktree_id: &str, worktree_path: &Path) {
    let drifts = match state.env_vault.detect_drift(&state.store, project_id, worktree_path) {
        Ok(d) => d,
        Err(err) => {
            tracing::debug!(%err, worktree = worktree_id, "drift detection failed");
            return;
        }
    };
    for drift in drifts {
        // The event payload is key names only, never any value (masked).
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::ENV_DRIFT,
            serde_json::json!({
                "worktree_id": worktree_id,
                "target": drift.target,
                "added_keys": drift.added_keys,
                "removed_keys": drift.removed_keys,
                "changed_keys": drift.changed_keys,
                "opaque_change": drift.opaque_change,
            }),
        );
        let score = needs_input_score(state, Some(card_id));
        let _ = state.store.raise_needs_you(
            card_id,
            "env_drift",
            &format!("env_drift:{worktree_id}:{}", drift.target),
            score,
        );
        tracing::info!(
            worktree = worktree_id,
            target = %drift.target,
            added = drift.added_keys.len(),
            removed = drift.removed_keys.len(),
            changed = drift.changed_keys.len(),
            "env drift raised"
        );
    }
}

// ---- session.* additions ----

/// `session.rename { session_id, title }`: persist the tab label (empty clears).
pub fn session_rename(state: &AppState, req: SessionRename) -> Result<Simple, ProtocolError> {
    let matched = state
        .store
        .set_session_title(&req.session_id, req.title.trim())
        .map_err(store_err)?;
    if !matched {
        return Err(ProtocolError::not_found(format!("session {}", req.session_id)));
    }
    Ok(Simple::ok())
}

/// `session.resume { session_id }`: relaunch an interrupted/ended session in the same
/// cwd/worktree with the harness resume flag (`architecture.md` / session resume). A
/// NEW session row is created, linked via `resumed_from`; hooks are re-wired so the
/// resumed session's harness-native id (reassigned on resume for claude) is
/// re-captured; a `session_resumed` event drives the UI scrollback divider.
pub fn session_resume(state: &AppState, req: SessionResume) -> Result<SessionResumed, ProtocolError> {
    let row = state
        .store
        .get_session(&req.session_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("session {}", req.session_id)))?;
    let resume_ref = row.resume_ref.clone().ok_or_else(|| {
        ProtocolError::bad_request(
            "session has no captured resume_ref; resume only via the harness's own most-recent mechanism",
        )
    })?;

    // The launcher command recorded on the row, else the adapter family name (the
    // built-in command, e.g. claude).
    let launcher = row.agent_id.as_deref().and_then(|aid| state.store.get_agent(aid).ok().flatten());
    let command = launcher.as_ref().map(|a| a.command.clone()).unwrap_or_else(|| row.harness.clone());
    let resume_argv = harness::resume_command(&row.harness, &command, &resume_ref).ok_or_else(|| {
        ProtocolError::bad_request(format!("harness '{}' declares no resume mechanism", row.harness))
    })?;

    // Same worktree (its lease survives a daemon restart) or the recorded cwd.
    let cwd = match row.worktree_id.as_deref().and_then(|w| state.store.get_worktree(w).ok().flatten()) {
        Some(wt) => Some(PathBuf::from(wt.path)),
        None => row.cwd.as_deref().map(PathBuf::from),
    };
    let mut env = BTreeMap::new();
    if let Some(agent) = &launcher {
        env.extend(agent.extra_env.clone());
    }

    let (command_argv, hook_token) = crate::hooks::wire_native_hooks(state, &row.harness, resume_argv);
    let card_ulid = row.card_id.as_deref().and_then(|c| Ulid::from_string(c).ok());
    let worktree_ulid = row.worktree_id.as_deref().and_then(|w| Ulid::from_string(w).ok());

    let spec = SessionSpec {
        harness: row.harness.clone(),
        command: command_argv,
        cols: DISPATCH_COLS,
        rows: DISPATCH_ROWS,
        cwd,
        env,
        card_id: card_ulid,
        project_id: row.project_id.clone(),
        worktree_id: worktree_ulid,
        agent_id: row.agent_id.clone(),
        model: row.model.clone(),
        effort: row.effort.clone(),
        first_prompt: row.first_prompt.clone(),
        title: row.title.clone(),
        resumed_from: Some(row.id.clone()),
        scrollback_dir: Some(state.data_dir.scrollback_dir()),
    };
    let session = state
        .sessions
        .create(spec)
        .map_err(|e| ProtocolError::internal(format!("could not resume session: {e}")))?;
    if let Some(token) = hook_token {
        state.hooks.register(token, session.id.to_string());
    }
    // Scrollback divider: mark the resume on the card timeline (if carded).
    if let Some(card_id) = &row.card_id {
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::SESSION_RESUMED,
            serde_json::json!({
                "from_session": row.id,
                "to_session": session.id.to_string(),
                "resume_ref": resume_ref,
            }),
        );
    }
    Ok(SessionResumed {
        session_id: session.id.to_string(),
        resumed_from: row.id,
        resume_ref: Some(resume_ref),
    })
}

/// `session.list {}`: the enriched fleet table. Persisted dispatch sessions come
/// from the store (including `interrupted` ones); live store-less sessions (Phase 0
/// style shells) are appended so nothing running is ever hidden.
pub fn session_list(state: &AppState) -> Result<SessionListResult, ProtocolError> {
    let now = now_ms();
    let mut sessions: Vec<SessionSummary> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for row in state.store.all_session_rows().map_err(store_err)? {
        seen.insert(row.id.clone());
        sessions.push(summarize(state, row));
    }

    // Live sessions with no DB row (created via bare session.create).
    for info in state.sessions.list() {
        if seen.contains(&info.session_id) {
            continue;
        }
        sessions.push(SessionSummary {
            session_id: info.session_id,
            card_id: None,
            project_id: None,
            project_name: None,
            harness: info.harness,
            agent: None,
            agent_id: None,
            title: None,
            status_note: None,
            state: if info.alive {
                session_state::WORKING.to_string()
            } else {
                session_state::DONE.to_string()
            },
            alive: info.alive,
            elapsed_ms: now.saturating_sub(info.created_at_ms as i64).max(0) as u64,
            resume_ref: None,
            first_prompt: None,
            created_at_ms: info.created_at_ms,
        });
    }

    sessions.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
    Ok(SessionListResult { sessions })
}

/// `fleet.status {}`: one snapshot of the fleet (`protocol.md` fleet.*): the
/// enriched session table plus open Needs You items, highest score first.
/// Gate runs join the payload with the gate engine (M5).
pub fn fleet_status(state: &AppState) -> Result<FleetStatusResult, ProtocolError> {
    let sessions = session_list(state)?.sessions;
    let needs_you = state
        .store
        .list_needs_you(true)
        .map_err(store_err)?
        .into_iter()
        .map(|i| NeedsYouItem {
            id: i.id,
            card_id: i.card_id,
            kind: i.kind,
            dedupe_key: i.dedupe_key,
            score: i.score,
            raised_at: i.raised_at,
            notified_at: i.notified_at,
            resolved_at: i.resolved_at,
            resolved_by: i.resolved_by,
        })
        .collect();
    Ok(FleetStatusResult { sessions, needs_you })
}

// ---- agents.* (configured launchers, `protocol.md` / session.*) ----

/// `agents.list {}`: every configured launcher with `detected_version`, `enabled`,
/// `source`, and computed `caution`.
pub fn agents_list(state: &AppState) -> Result<AgentsListResult, ProtocolError> {
    let agents = state.store.list_agents().map_err(store_err)?;
    Ok(AgentsListResult { agents })
}

/// `agents.add { name, adapter, command, extra_args, extra_env }`: create a custom
/// launcher after validating name uniqueness, a non-empty command, and a known
/// adapter (`extra_args`/`extra_env` shape is enforced by the wire types).
pub fn agents_add(state: &AppState, req: AgentAdd) -> Result<AgentResult, ProtocolError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ProtocolError::bad_request("agent name must not be empty"));
    }
    let command = req.command.trim();
    if command.is_empty() {
        return Err(ProtocolError::bad_request("agent command must not be empty"));
    }
    if !harness::is_known_adapter(&req.adapter) {
        return Err(ProtocolError::bad_request(format!(
            "adapter '{}' is not known (one of: {})",
            req.adapter,
            harness::KNOWN_ADAPTERS.join(", ")
        )));
    }
    if state.store.get_agent_by_name(name).map_err(store_err)?.is_some() {
        return Err(ProtocolError::bad_request(format!("an agent named '{name}' already exists")));
    }
    let agent = state
        .store
        .insert_agent(NewAgent {
            name: name.to_string(),
            adapter: req.adapter,
            command: command.to_string(),
            extra_args: req.extra_args,
            extra_env: req.extra_env,
            source: agent_source::CUSTOM.to_string(),
            detected_version: None,
            enabled: true,
        })
        .map_err(store_err)?;
    Ok(AgentResult { agent })
}

/// `agents.update { id, ... }`: patch a launcher (id or name). Absent fields are
/// unchanged; `enabled` toggles it. Validates a changed name stays unique, a changed
/// adapter is known, and a changed command is non-empty.
pub fn agents_update(state: &AppState, req: AgentUpdate) -> Result<AgentResult, ProtocolError> {
    let existing = state
        .store
        .resolve_agent(&req.id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("agent '{}'", req.id)))?;
    if let Some(adapter) = &req.adapter {
        if !harness::is_known_adapter(adapter) {
            return Err(ProtocolError::bad_request(format!(
                "adapter '{adapter}' is not known (one of: {})",
                harness::KNOWN_ADAPTERS.join(", ")
            )));
        }
    }
    let command = match &req.command {
        Some(c) if c.trim().is_empty() => {
            return Err(ProtocolError::bad_request("agent command must not be empty"))
        }
        Some(c) => Some(c.trim().to_string()),
        None => None,
    };
    let name = match &req.name {
        Some(n) if n.trim().is_empty() => {
            return Err(ProtocolError::bad_request("agent name must not be empty"))
        }
        Some(n) => {
            let n = n.trim();
            if let Some(other) = state.store.get_agent_by_name(n).map_err(store_err)? {
                if other.id != existing.id {
                    return Err(ProtocolError::bad_request(format!(
                        "an agent named '{n}' already exists"
                    )));
                }
            }
            Some(n.to_string())
        }
        None => None,
    };
    let agent = state
        .store
        .update_agent(
            &existing.id,
            AgentPatch {
                name,
                adapter: req.adapter,
                command,
                extra_args: req.extra_args,
                extra_env: req.extra_env,
                enabled: req.enabled,
            },
        )
        .map_err(store_err)?;
    Ok(AgentResult { agent })
}

/// `agents.remove { id }`: remove a launcher (id or name). The store refuses while a
/// non-ended session references it, suggesting disable instead.
pub fn agents_remove(state: &AppState, req: AgentRemove) -> Result<AgentRemoved, ProtocolError> {
    let existing = state
        .store
        .resolve_agent(&req.id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("agent '{}'", req.id)))?;
    let removed = state.store.remove_agent(&existing.id).map_err(store_err)?;
    Ok(AgentRemoved { ok: true, removed: removed.name })
}

/// `agents.detect {}`: scan PATH for the known CLIs and upsert detected launchers,
/// returning what was found this run plus the refreshed list. Runs only on this
/// explicit call, never in the background (`product.md` / Autodetection).
pub fn agents_detect(state: &AppState) -> Result<AgentsDetected, ProtocolError> {
    let found = agents::detect_installed();
    let outcome = state.store.apply_detection(found).map_err(store_err)?;
    let found = outcome
        .found
        .into_iter()
        .map(|(cli, created)| DetectedCli {
            name: cli.name,
            command: cli.command,
            version: cli.version,
            created,
        })
        .collect();
    let agents = state.store.list_agents().map_err(store_err)?;
    Ok(AgentsDetected { found, agents })
}

/// A launcher resolved for a bare interactive terminal (`session.create` with an
/// `agent`): the interactive argv (no brief) and the launcher's extra env. Bare
/// sessions are not persisted, so the launcher id is not recorded on a row here.
pub struct AgentLaunch {
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub harness: String,
    /// The resolved launcher id, recorded on the persisted bare-session row.
    pub agent_id: Option<String>,
}

/// Resolve a `session.create` `agent` to an interactive launch (`product.md` /
/// Settings > Agents): the launcher's command + adapter flags + extra args, with no
/// brief, plus its extra env. The launcher must be enabled.
pub fn resolve_agent_launch(state: &AppState, agent_ref: &str) -> Result<AgentLaunch, ProtocolError> {
    let agent = state
        .store
        .resolve_agent(agent_ref)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("agent '{agent_ref}'")))?;
    if !agent.enabled {
        return Err(ProtocolError::bad_request(format!(
            "launcher '{}' is disabled; enable it before use",
            agent.name
        )));
    }
    let command = harness::launcher_interactive_command(
        &agent.adapter,
        &agent.command,
        &agent.extra_args,
        None,
        None,
    );
    Ok(AgentLaunch { command, env: agent.extra_env, harness: agent.adapter, agent_id: Some(agent.id) })
}

/// Match a working directory to a registered project (`cwd -> project`, Phase 2 API
/// reconciliation): the project whose canonical path is a prefix of `cwd`, preferring
/// the deepest match. Used to give a cardless (bare `session.create`) session its
/// Projects-tree home so it survives a daemon restart under the right project.
pub fn find_project_for_path(state: &AppState, cwd: &Path) -> Option<String> {
    let target = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut best: Option<(usize, String)> = None;
    for project in state.store.list_projects().ok()? {
        let proot = std::fs::canonicalize(&project.path).unwrap_or_else(|_| PathBuf::from(&project.path));
        if target.starts_with(&proot) {
            let depth = proot.components().count();
            if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                best = Some((depth, project.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// How long to wait for a fresh session's composer to be ready before submitting the
/// New Session first prompt through verified submit.
const FIRST_PROMPT_READY_TIMEOUT: Duration = Duration::from_secs(20);
/// Needs You score for a first-prompt submit failure (an active blocked agent).
const FIRST_PROMPT_FAIL_SCORE: i64 = 500;

/// Submit the New Session first prompt through verified submit once the composer is
/// ready (`adapters.md` / Verified submit; Phase 2 first-prompt auto-submit).
///
/// Runs on a background thread because verified submit blocks (typing, popup settle,
/// redraw waits). A failed submit raises Needs You for a carded session, or is logged
/// for a cardless one, so the message is never silently dropped.
pub fn spawn_first_prompt_submit(state: &AppState, session: Arc<Session>, prompt: String) {
    let store = Arc::clone(&state.store);
    let harness = session.harness.clone();
    let handle = std::thread::Builder::new()
        .name(format!("first-prompt-{}", session.id))
        .spawn(move || {
            let manifest = match bundled_manifests().get(&harness) {
                Some(m) => m,
                None => {
                    tracing::warn!(%harness, "no manifest; first prompt not auto-submitted");
                    return;
                }
            };
            let cfg = SubmitConfig::from_manifest(manifest);
            if !steer::wait_for_composer_ready(&session, manifest, FIRST_PROMPT_READY_TIMEOUT) {
                tracing::warn!(session_id = %session.id, "composer not ready; first prompt not submitted");
                raise_first_prompt_failure(&store, &session);
                return;
            }
            let outcome = steer::send_verified(&session, manifest, &prompt, &cfg);
            if outcome.submitted {
                tracing::info!(session_id = %session.id, attempts = outcome.attempts, "first prompt submitted");
            } else {
                tracing::warn!(session_id = %session.id, attempts = outcome.attempts, "first prompt verified submit failed");
                raise_first_prompt_failure(&store, &session);
            }
        });
    if let Err(err) = handle {
        tracing::warn!(%err, "could not spawn first-prompt submit thread");
    }
}

/// How long after launch to watch for and answer a trust dialog (`adapters.md` /
/// dispatch flow step 7: "watch for trust dialogs within the first N seconds").
const TRUST_WATCH_WINDOW: Duration = Duration::from_secs(20);

/// Watch a freshly launched session for a trust/permission dialog and answer it once
/// per the manifest (`adapters.md` / dialogs, dispatch flow step 7). Runs on a
/// background thread for a bounded window; a harness with no trust rule is a no-op.
pub fn spawn_trust_watcher(session: Arc<Session>, harness: String) {
    let manifest = match bundled_manifests().get(&harness) {
        Some(m) => m,
        None => return,
    };
    let rule = match &manifest.dialogs.trust {
        Some(r) => r.clone(),
        None => return,
    };
    let _ = std::thread::Builder::new().name(format!("trust-watch-{}", session.id)).spawn(move || {
        let deadline = Instant::now() + TRUST_WATCH_WINDOW;
        while Instant::now() < deadline {
            if !session.is_alive() {
                return;
            }
            let screen = session.capture_plain();
            if dflow_core::heuristics::is_trust_dialog(&harness, &screen) {
                let bytes = trust_response_bytes(&rule.response);
                if session.write_input(&bytes).is_ok() {
                    tracing::info!(session_id = %session.id, "answered trust dialog per manifest");
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    });
}

/// Map a manifest dialog response keyword to the bytes to send.
fn trust_response_bytes(response: &str) -> Vec<u8> {
    match response {
        "enter" => b"\r".to_vec(),
        "y" | "yes" => b"y\r".to_vec(),
        other => other.as_bytes().to_vec(),
    }
}

/// Needs You score v0 = priority + age (`data-model.md` / needs_you_items). Priority
/// dominates; age in minutes breaks ties so older asks float up. A cardless session has
/// no card, so it scores from age alone (0, since it raises no item). Shared by the
/// tier-3 supervisor and the tier-2 hook endpoint.
pub fn needs_input_score(state: &AppState, card_id: Option<&str>) -> i64 {
    let (priority, created_at) = match card_id.and_then(|c| state.store.get_card(c).ok().flatten()) {
        Some(card) => (card.priority, card.created_at),
        None => (0, 0),
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let age_minutes = if created_at > 0 { ((now - created_at) / 60_000).max(0) } else { 0 };
    priority * 100 + age_minutes
}

/// Raise a Needs You for a failed first-prompt submit on a carded session.
fn raise_first_prompt_failure(store: &Store, session: &Session) {
    if let Ok(Some(row)) = store.get_session(&session.id.to_string()) {
        if let Some(card_id) = row.card_id {
            let dedupe = format!("first_prompt:{}", session.id);
            let _ = store.raise_needs_you(&card_id, "agent_blocked", &dedupe, FIRST_PROMPT_FAIL_SCORE);
        }
    }
}

/// Build a `SessionSummary` from a persisted row plus live-manager facts.
///
/// Project linkage resolves from the card when the session is carded, else from the
/// row's own `project_id` (the cwd->project match captured for a cardless session).
fn summarize(state: &AppState, row: SessionRow) -> SessionSummary {
    let alive = state
        .sessions
        .get_str(&row.id)
        .map(|s| s.is_alive())
        .unwrap_or(false);
    let project = row
        .card_id
        .as_deref()
        .and_then(|cid| state.store.get_card(cid).ok().flatten())
        .and_then(|c| c.project_id)
        .or_else(|| row.project_id.clone())
        .and_then(|pid| state.store.get_project(&pid).ok().flatten());
    let (project_id, project_name) =
        project.map(|p| (Some(p.id), Some(p.name))).unwrap_or((None, None));
    // Join the launcher name for the fleet table, so two launchers in one adapter
    // family are distinguishable (Phase 1.5 debt, closed here).
    let agent = row
        .agent_id
        .as_deref()
        .and_then(|aid| state.store.get_agent(aid).ok().flatten())
        .map(|a| a.name);
    let end = row.ended_at.unwrap_or_else(now_ms);
    SessionSummary {
        session_id: row.id,
        card_id: row.card_id,
        project_id,
        project_name,
        harness: row.harness,
        agent,
        agent_id: row.agent_id,
        title: row.title,
        status_note: row.status_note,
        state: row.state,
        alive,
        elapsed_ms: (end - row.created_at).max(0) as u64,
        resume_ref: row.resume_ref,
        first_prompt: row.first_prompt,
        created_at_ms: row.created_at.max(0) as u64,
    }
}

// ---- M2 agent CLI: token-scoped surfaces (`agent-cli.md`, `knowledge.md`) ----

/// Inject the agent-CLI environment into a session's spawn env: the per-task token,
/// its card, the WS endpoint, and `dflow` prepended to PATH so the CLI is runnable
/// (`agent-cli.md` / Authentication and wiring; `adapters.md` dispatch flow step 5).
pub fn inject_agent_env(
    state: &AppState,
    env: &mut BTreeMap<String, String>,
    token: &str,
    card_id: Option<&str>,
) {
    env.insert("DFLOW_TOKEN".to_string(), token.to_string());
    env.insert("DFLOW_CARD".to_string(), card_id.unwrap_or("").to_string());
    let port = state.http_port.load(Ordering::SeqCst);
    if port != 0 {
        env.insert("DFLOW_ENDPOINT".to_string(), format!("ws://127.0.0.1:{port}/ws"));
    }
    if let Some(dir) = crate::tokens::dflow_binary_dir() {
        let sep = if cfg!(windows) { ';' } else { ':' };
        // Reuse the exact existing PATH key casing (`Path` on Windows) so the override
        // lands on the same variable. Crucially we ALWAYS set the override, not just
        // when the dir is missing: portable-pty rebuilds the child env from the Windows
        // registry (not the daemon's process env), so an unset override would drop the
        // daemon's PATH entirely and `dflow` would be unreachable. Setting it replaces
        // that registry base with the daemon's full PATH plus the dflow dir.
        let path_key = env
            .keys()
            .find(|k| k.eq_ignore_ascii_case("PATH"))
            .cloned()
            .or_else(|| {
                std::env::vars_os().find_map(|(k, _)| {
                    let k = k.to_string_lossy().into_owned();
                    k.eq_ignore_ascii_case("PATH").then_some(k)
                })
            })
            .unwrap_or_else(|| "PATH".to_string());
        let existing =
            env.get(&path_key).cloned().or_else(|| std::env::var(&path_key).ok()).unwrap_or_default();
        let dir_str = dir.to_string_lossy().into_owned();
        let value = if existing.split(sep).any(|p| p == dir_str) {
            existing
        } else {
            format!("{dir_str}{sep}{existing}")
        };
        env.insert(path_key, value);
    } else {
        tracing::warn!("dflow binary not found next to dflowd; PATH not prepended");
    }
}

/// `agent.context {}`: the one read behind bare `dflow` and `dflow card`. Resolves the
/// current card, session state/note, acceptance criteria, and the project digest from
/// the token scope (`agent-cli.md` / Verbs).
pub fn agent_context(state: &AppState, token: &AgentToken) -> Result<AgentContextResult, ProtocolError> {
    let session_row = match token.session_id() {
        Some(sid) => state.store.get_session(&sid).map_err(store_err)?,
        None => None,
    };
    // The effective card: the token's dispatch card, else the card a cardless New Session
    // adopted via `dflow card create` (persisted on its session row), so bare `dflow`
    // shows the adopted card instead of "no card assigned" once work is on the board.
    let effective_card_id = token
        .card_id
        .clone()
        .or_else(|| session_row.as_ref().and_then(|r| r.card_id.clone()));
    let card = match &effective_card_id {
        Some(cid) => state.store.get_card(cid).map_err(store_err)?,
        None => None,
    };
    let (session_state, status_note) = match &session_row {
        Some(row) => (Some(row.state.clone()), row.status_note.clone()),
        None => (None, None),
    };
    let acceptance = card
        .as_ref()
        .and_then(|c| c.brief.as_deref())
        .map(parse_acceptance)
        .unwrap_or_default();
    let project_id = token
        .project_id
        .clone()
        .or_else(|| card.as_ref().and_then(|c| c.project_id.clone()));
    let (digest, knowledge_notes, project_name) = match &project_id {
        Some(pid) => match resolve_knowledge_dir(state, pid)? {
            Some((dir, name)) => (
                knowledge::read_digest(&dir),
                state.store.knowledge_count(pid).unwrap_or(0),
                Some(name),
            ),
            None => (None, 0, None),
        },
        None => (None, 0, None),
    };
    Ok(AgentContextResult {
        card,
        project_name,
        session_id: token.session_id(),
        session_state,
        status_note,
        acceptance,
        digest,
        knowledge_notes,
    })
}

/// `session.self_report { state, note? }`: tier-1 lifecycle self-report (`dflow
/// status`). `done` is a stage-advance request the daemon arbitrates against the
/// dispatch recipe's stage list (`agent-cli.md` / Stage advancement arbitration).
pub fn session_self_report(
    state: &AppState,
    token: &AgentToken,
    req: SelfReport,
) -> Result<SelfReportResult, ProtocolError> {
    let session_id = token
        .session_id()
        .ok_or_else(|| ProtocolError::forbidden("token is not bound to a session yet"))?;
    let note = req.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    match req.state.as_str() {
        "working" => {
            state.store.agent_report_working(&session_id, note).map_err(store_err)?;
            Ok(SelfReportResult {
                recorded: session_state::WORKING.to_string(),
                advanced: false,
                note_set: note.is_some(),
                blocked_reason: None,
                next: None,
            })
        }
        "blocked" => {
            let note = note.ok_or_else(|| {
                ProtocolError::bad_request("`dflow status blocked` requires a note explaining the block")
            })?;
            let score = needs_input_score(state, token.card_id.as_deref());
            state.store.agent_report_blocked(&session_id, note, score).map_err(store_err)?;
            Ok(SelfReportResult {
                recorded: session_state::BLOCKED.to_string(),
                advanced: false,
                note_set: true,
                blocked_reason: None,
                next: None,
            })
        }
        "done" => {
            // Arbitration (`agent-cli.md`): the recipe's stage list supplies the
            // conditions. In this milestone none of the gateable conditions are
            // enforceable machinery yet (plan approval needs Plan Studio, the gate
            // engine is M5), so `done` advances; the response tells the agent what the
            // recipe's flow does next instead of a generic line, and says honestly
            // when the remaining stages await the human.
            let next = token.recipe.as_deref().map(recipe_done_next);
            state.store.agent_report_done(&session_id, note).map_err(store_err)?;
            Ok(SelfReportResult {
                recorded: session_state::DONE.to_string(),
                advanced: true,
                note_set: note.is_some(),
                blocked_reason: None,
                next,
            })
        }
        other => Err(ProtocolError::bad_request(format!(
            "unknown status '{other}' (expected working|blocked|done)"
        ))),
    }
}

/// The recipe-aware `next:` line for a granted `done`: what the dispatch recipe's
/// stage list says follows the implement stage (`recipes.md`: the stage list drives
/// arbitration; `roadmap.md` M2 interim: checks-only verify, human-approved push).
fn recipe_done_next(recipe: &Recipe) -> String {
    let after: Vec<&Stage> = recipe
        .stages
        .iter()
        .skip_while(|s| **s != Stage::Implement)
        .filter(|s| **s != Stage::Implement)
        .collect();
    if after.is_empty() {
        return format!(
            "recipe {} v{} ends at implement; the card completes from here",
            recipe.name, recipe.version
        );
    }
    let mut parts: Vec<String> = Vec::new();
    for stage in after {
        match stage {
            Stage::Verify => {
                let gate = recipe
                    .verify
                    .as_ref()
                    .map(|v| v.gate)
                    .unwrap_or(dflow_core::recipe::GateStrictness::Full);
                parts.push(format!("verify ({})", gate_str(gate)));
            }
            Stage::Ship => {
                let target = recipe
                    .ship
                    .as_ref()
                    .map(|s| s.target)
                    .unwrap_or(dflow_core::recipe::ShipTarget::Pr);
                parts.push(format!("ship ({})", ship_str(target)));
            }
            other => parts.push(other.as_str().to_string()),
        }
    }
    format!(
        "recipe {} v{}: {} remain; the captain drives them until the gate engine lands",
        recipe.name,
        recipe.version,
        parts.join(" then ")
    )
}

fn gate_str(gate: dflow_core::recipe::GateStrictness) -> &'static str {
    match gate {
        dflow_core::recipe::GateStrictness::Full => "full",
        dflow_core::recipe::GateStrictness::ChecksOnly => "checks_only",
        dflow_core::recipe::GateStrictness::None => "none",
    }
}

fn ship_str(target: dflow_core::recipe::ShipTarget) -> &'static str {
    match target {
        dflow_core::recipe::ShipTarget::Pr => "pr",
        dflow_core::recipe::ShipTarget::LocalMerge => "local_merge",
        dflow_core::recipe::ShipTarget::None => "none",
    }
}

/// `session.set_note { note }`: set the session-strip status note (`dflow card note`).
pub fn session_set_note(state: &AppState, token: &AgentToken, req: SetNote) -> Result<Simple, ProtocolError> {
    let session_id = token
        .session_id()
        .ok_or_else(|| ProtocolError::forbidden("token is not bound to a session yet"))?;
    let note = req.note.trim();
    if note.is_empty() {
        return Err(ProtocolError::bad_request("note must not be empty"));
    }
    let matched = state.store.set_session_status_note(&session_id, note).map_err(store_err)?;
    if !matched {
        return Err(ProtocolError::not_found("session"));
    }
    Ok(Simple::ok())
}

/// `card.create` from an agent token: enforce the card budget, stamp origin from the
/// token (audit-scoped -> `audit`, else `manual`) with the fingerprint as the dedupe
/// ref, associate the token's project, and record ownership for later updates.
pub fn card_create_scoped(
    state: &AppState,
    token: &AgentToken,
    req: CardCreate,
) -> Result<CardCreated, ProtocolError> {
    if req.title.trim().is_empty() {
        return Err(ProtocolError::bad_request("card title must not be empty"));
    }
    let project_id = req.project_id.or_else(|| token.project_id.clone());
    if let Some(pid) = &project_id {
        if state.store.get_project(pid).map_err(store_err)?.is_none() {
            return Err(ProtocolError::not_found(format!("project {pid}")));
        }
    }
    // Audit runs stamp `origin: audit` automatically and always file into Inbox.
    let origin_kind = if token.audit { "audit" } else { "manual" }.to_string();
    let lane = if token.audit { "inbox".to_string() } else { req.lane.unwrap_or_else(|| "inbox".into()) };
    let origin_ref = req.fingerprint.filter(|f| !f.trim().is_empty());

    // Durable audit-dismissal dedupe (`data-model.md` / UNIQUE(origin_kind, origin_ref)):
    // a re-audit with a fingerprint that already exists is a refresh, not a refile; a
    // fingerprint the human dismissed is suppressed. Only a genuine new filing consumes
    // the card budget.
    if let Some(oref) = &origin_ref {
        if let Some((existing, dismissed)) =
            state.store.get_card_by_origin(&origin_kind, oref).map_err(store_err)?
        {
            if dismissed {
                // The human dismissed this finding; do not refile it.
                return Ok(CardCreated {
                    card_id: existing.id.clone(),
                    card: existing,
                    dedupe: Some("suppressed".to_string()),
                });
            }
            // Refresh the existing finding in place (no budget cost).
            let card = state
                .store
                .update_card(
                    &existing.id,
                    CardPatch {
                        title: Some(req.title),
                        card_type: Some(req.card_type),
                        dial_recipe: None,
                        brief: req.brief.or(existing.brief),
                        priority: Some(req.priority.unwrap_or(existing.priority)),
                    },
                )
                .map_err(store_err)?;
            return Ok(CardCreated { card_id: card.id.clone(), card, dedupe: Some("refreshed".to_string()) });
        }
    }

    // A genuine new filing: budget-gate, then create.
    if !token.card_budget_ok() {
        let cap = token.budget_cards.unwrap_or(0);
        return Err(ProtocolError::budget_exceeded(format!(
            "card budget reached ({cap}); put the remaining cards in your report"
        )));
    }
    let card = state
        .store
        .create_card(NewCard {
            project_id,
            card_type: req.card_type,
            title: req.title,
            lane,
            dial_recipe: req.dial_recipe,
            brief: req.brief,
            priority: req.priority.unwrap_or(0),
            origin_kind,
            origin_ref,
            origin_data: None,
        })
        .map_err(store_err)?;
    token.record_created_card(&card.id);
    // A cardless New Session adopts its FIRST created card as the session's card, so the
    // board shows the session under it and bare `dflow` / `dflow status` resolve to it
    // going forward (`agent-cli.md`: `dflow card create` sets the session's card). Only
    // fills an empty link (first card wins); a dispatched session has `card_id` set and is
    // skipped, so its follow-up cards stay follow-ups.
    if token.card_id.is_none() {
        if let Some(session_id) = token.session_id() {
            let _ = state.store.set_session_card(&session_id, &card.id);
        }
    }
    Ok(CardCreated { card_id: card.id.clone(), card, dedupe: Some("created".to_string()) })
}

/// `card.update` from an agent token: only on a card the token owns.
pub fn card_update_scoped(
    state: &AppState,
    token: &AgentToken,
    req: CardUpdate,
) -> Result<CardResult, ProtocolError> {
    ensure_card_owned(token, &req.card_id)?;
    card_update(state, req)
}

/// `card.move` from an agent token: only on an owned card, and never for an audit token
/// (an audit files into Inbox but may not advance its own filings; `security.md`).
pub fn card_move_scoped(
    state: &AppState,
    token: &AgentToken,
    req: CardMove,
) -> Result<CardResult, ProtocolError> {
    ensure_card_owned(token, &req.card_id)?;
    if token.audit {
        return Err(ProtocolError::forbidden(
            "audit-scoped tokens cannot move lanes on cards they created",
        ));
    }
    card_move(state, req)
}

/// `card.get` from an agent token: only on an owned card.
pub fn card_get_scoped(
    state: &AppState,
    token: &AgentToken,
    req: CardGet,
) -> Result<CardGetResult, ProtocolError> {
    ensure_card_owned(token, &req.card_id)?;
    card_get(state, req)
}

/// Reject a card operation whose target is outside the token's scope (`security.md` /
/// Per-task tokens): its own dispatch card or a card it created.
fn ensure_card_owned(token: &AgentToken, card_id: &str) -> Result<(), ProtocolError> {
    if token.owns_card(card_id) {
        Ok(())
    } else {
        Err(ProtocolError::forbidden(format!(
            "card {card_id} is outside this task's scope"
        )))
    }
}

/// Resolve the knowledge dir + project name for a project id, or `None` if unregistered.
fn resolve_knowledge_dir(
    state: &AppState,
    project_id: &str,
) -> Result<Option<(PathBuf, String)>, ProtocolError> {
    let project = match state.store.get_project(project_id).map_err(store_err)? {
        Some(p) => p,
        None => return Ok(None),
    };
    let over = state.store.project_knowledge_path(project_id).map_err(store_err)?;
    let dir = knowledge::resolve_dir(Path::new(&project.path), over.as_deref());
    Ok(Some((dir, project.name)))
}

/// The project a `know.*` request targets: the explicit `project_id` (root clients) or
/// the token's project (agent clients). Errors when neither is available.
fn know_project(
    token: Option<&AgentToken>,
    explicit: Option<String>,
) -> Result<String, ProtocolError> {
    explicit
        .or_else(|| token.and_then(|t| t.project_id.clone()))
        .ok_or_else(|| ProtocolError::bad_request("no project in scope for a knowledge verb"))
}

/// `know.index {}`: digest + catalog counts (`dflow know`).
pub fn know_index(
    state: &AppState,
    token: Option<&AgentToken>,
    req: KnowIndex,
) -> Result<KnowIndexResult, ProtocolError> {
    let project_id = know_project(token, req.project_id)?;
    let (digest, digest_lines, project_name) = match resolve_knowledge_dir(state, &project_id)? {
        Some((dir, name)) => {
            let digest = knowledge::read_digest(&dir);
            let lines = digest.as_deref().map(|d| d.lines().count() as u32).unwrap_or(0);
            (digest, lines, Some(name))
        }
        None => (None, 0, None),
    };
    let catalog = state
        .store
        .knowledge_catalog(&project_id)
        .map_err(store_err)?
        .into_iter()
        .map(|(note_type, count)| KnowCatalogGroup { note_type, count })
        .collect::<Vec<_>>();
    let total_notes = catalog.iter().map(|g| g.count).sum();
    Ok(KnowIndexResult { project_name, digest, digest_lines, catalog, total_notes })
}

/// `know.find { query, type? }`: substring/tag search (`dflow know find`).
pub fn know_find(
    state: &AppState,
    token: Option<&AgentToken>,
    req: KnowFind,
) -> Result<KnowFindResult, ProtocolError> {
    let project_id = know_project(token, req.project_id)?;
    if req.query.trim().is_empty() {
        return Err(ProtocolError::bad_request("find needs a non-empty query"));
    }
    let rows = state
        .store
        .find_knowledge(&project_id, &req.query, req.note_type.as_deref())
        .map_err(store_err)?;
    let notes = rows
        .into_iter()
        .take(KNOW_FIND_LIMIT)
        .map(|r| KnowNoteHit {
            id: r.id,
            note_type: r.note_type,
            description: r.description.unwrap_or_default(),
        })
        .collect();
    Ok(KnowFindResult { notes })
}

/// `know.get { id, full }`: print one note, truncated unless `full` (`dflow know get`).
pub fn know_get(
    state: &AppState,
    token: Option<&AgentToken>,
    req: KnowGet,
) -> Result<KnowGetResult, ProtocolError> {
    let project_id = know_project(token, req.project_id)?;
    let dir = match resolve_knowledge_dir(state, &project_id)? {
        Some((dir, _)) => dir,
        None => return Ok(KnowGetResult { note: None }),
    };
    let parsed = match knowledge::read_note(&dir, &req.id) {
        Some(p) => p,
        None => return Ok(KnowGetResult { note: None }),
    };
    let total_lines = parsed.body.lines().count() as u32;
    let (body, truncated) = if req.full || (total_lines as usize) <= KNOW_GET_LINE_CAP {
        (parsed.body, false)
    } else {
        let head: Vec<&str> = parsed.body.lines().take(KNOW_GET_LINE_CAP).collect();
        (head.join("\n"), true)
    };
    Ok(KnowGetResult {
        note: Some(KnowNote {
            id: req.id,
            note_type: parsed.note_type,
            title: parsed.title,
            body,
            truncated,
            total_lines,
        }),
    })
}

/// `know.add { type, title, body, tags? }`: write a note to the PROJECT ROOT checkout
/// via the daemon (never the worktree diff), rebuild the index, regenerate the Catalog,
/// and append a `knowledge_updated` event (`knowledge.md` / Write path mechanics). The
/// note budget is enforced for a budgeted task token.
pub fn know_add(
    state: &AppState,
    token: Option<&AgentToken>,
    req: KnowAdd,
) -> Result<KnowAddResult, ProtocolError> {
    if req.note_type.trim().is_empty() {
        return Err(ProtocolError::bad_request("note type must not be empty"));
    }
    if req.title.trim().is_empty() {
        return Err(ProtocolError::bad_request("note title must not be empty"));
    }
    if let Some(t) = token {
        if !t.note_budget_ok() {
            let cap = t.budget_notes.unwrap_or(0);
            return Err(ProtocolError::budget_exceeded(format!(
                "note budget reached ({cap}); put the remaining notes in your report"
            )));
        }
    }
    let project_id = know_project(token, req.project_id)?;
    let (dir, project_name) = resolve_knowledge_dir(state, &project_id)?
        .ok_or_else(|| ProtocolError::not_found(format!("project {project_id}")))?;

    let card = token.and_then(|t| t.card_id.clone());
    let outcome = knowledge::write_note(
        &dir,
        req.note_type.trim(),
        req.title.trim(),
        &req.body,
        &req.tags,
        card.as_deref(),
    )
    .map_err(|e| ProtocolError::internal(format!("writing note: {e}")))?;

    // Rebuild the index from disk and regenerate the deterministic Catalog section.
    let notes = knowledge::scan(&dir);
    state.store.rebuild_knowledge_index(&project_id, &notes).map_err(store_err)?;
    knowledge::regenerate_catalog(&dir, &project_name, &notes)
        .map_err(|e| ProtocolError::internal(format!("regenerating catalog: {e}")))?;

    // Evidence on the timeline: a knowledge_updated event on the current card, if any.
    if let Some(card_id) = &card {
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::KNOWLEDGE_UPDATED,
            serde_json::json!({
                "path": outcome.rel_path,
                "type": req.note_type.trim(),
                "title": req.title.trim(),
                "verb": "add",
            }),
        );
    }
    if let Some(t) = token {
        t.record_note();
    }
    Ok(KnowAddResult { id: outcome.id, path: outcome.rel_path, created: outcome.created })
}

/// `notify.forward { payload }`: the codex notify bridge (`dflow notify-forward`). The
/// token resolves the session; the daemon parses the codex payload and applies the
/// lifecycle transition + resume-ref capture (`crate::hooks::apply_codex_notify`).
pub fn notify_forward(
    state: &AppState,
    token: &AgentToken,
    req: NotifyForward,
) -> Result<Simple, ProtocolError> {
    let session_id = token
        .session_id()
        .ok_or_else(|| ProtocolError::forbidden("token is not bound to a session yet"))?;
    crate::hooks::apply_codex_notify(state, &session_id, &req.payload);
    Ok(Simple::ok())
}

// ---- env.* (the env vault, `environments.md`, `protocol.md` / env.*) ----

/// Map a vault error onto the wire error taxonomy.
pub fn env_err(err: dflow_core::EnvError) -> ProtocolError {
    use dflow_core::EnvError;
    match err {
        EnvError::Invalid(what) => ProtocolError::bad_request(what),
        EnvError::Store(e) => store_err(e),
        EnvError::Crypto(e) => ProtocolError::internal(e.to_string()),
        EnvError::Io(e) => ProtocolError::internal(e.to_string()),
    }
}

/// Project vault entry metadata -> the wire shape (names and kinds only, never a value).
fn env_entry_info(m: dflow_core::EnvEntryMeta) -> EnvEntryInfo {
    EnvEntryInfo { key: m.key, kind: m.kind, target: m.target, version: m.version, updated_at: m.updated_at }
}

/// `env.set { project_id, key, value, kind, target? }`: seal and upsert. The value is
/// write-only; the response carries metadata only (`protocol.md`).
pub fn env_set(state: &AppState, req: EnvSet) -> Result<EnvSetResult, ProtocolError> {
    if state.store.get_project(&req.project_id).map_err(store_err)?.is_none() {
        return Err(ProtocolError::not_found(format!("project {}", req.project_id)));
    }
    let meta = state
        .env_vault
        .set_entry(&state.store, &req.project_id, &req.key, &req.kind, &req.value, req.target.as_deref())
        .map_err(env_err)?;
    Ok(EnvSetResult { entry: env_entry_info(meta), secure_at_rest: state.env_vault.crypto_is_secure() })
}

/// `env.list { project_id }`: names and kinds only, never values.
pub fn env_list(state: &AppState, req: EnvList) -> Result<EnvListResult, ProtocolError> {
    let entries = state
        .env_vault
        .list_entries(&state.store, &req.project_id)
        .map_err(env_err)?
        .into_iter()
        .map(env_entry_info)
        .collect();
    Ok(EnvListResult { entries })
}

/// `env.delete { project_id, key }`: remove a vault entry.
pub fn env_delete(state: &AppState, req: EnvDelete) -> Result<EnvDeleted, ProtocolError> {
    let deleted = state.env_vault.delete_entry(&state.store, &req.project_id, &req.key).map_err(env_err)?;
    Ok(EnvDeleted { deleted })
}

/// `env.materialize { worktree_id }`: decrypt the worktree's project vault into it
/// (daemon-internal; exposed for diagnostics). Registers the materialized secrets under
/// the worktree id so a later peek/event scrub sees them.
pub fn env_materialize(state: &AppState, req: EnvMaterialize) -> Result<EnvMaterialized, ProtocolError> {
    let wt = state
        .store
        .get_worktree(&req.worktree_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("worktree {}", req.worktree_id)))?;
    let mat = state
        .env_vault
        .materialize(&state.store, &wt.project_id, &PathBuf::from(&wt.path))
        .map_err(env_err)?;
    if !mat.secret_values.is_empty() {
        dflow_core::secret::registry().register(&req.worktree_id, mat.secret_values);
    }
    Ok(EnvMaterialized { vars: mat.vars, secrets: mat.secrets, files: mat.file_targets })
}

/// `env.cleanup { worktree_id }`: shred the worktree's materialized secret files
/// (daemon-internal; exposed for diagnostics) and drop its registered secrets.
pub fn env_cleanup(state: &AppState, req: EnvCleanup) -> Result<EnvCleaned, ProtocolError> {
    let wt = state
        .store
        .get_worktree(&req.worktree_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("worktree {}", req.worktree_id)))?;
    let shredded = state
        .env_vault
        .cleanup(&state.store, &wt.project_id, &PathBuf::from(&wt.path))
        .map_err(env_err)?;
    dflow_core::secret::registry().unregister(&req.worktree_id);
    Ok(EnvCleaned { shredded })
}

/// `env.import { project_id, path }`: parse a `.env` file, classify each key, and ingest
/// it (`environments.md` / Import assist). Reports what it did, never any value.
pub fn env_import(state: &AppState, req: EnvImport) -> Result<EnvImportResult, ProtocolError> {
    if state.store.get_project(&req.project_id).map_err(store_err)?.is_none() {
        return Err(ProtocolError::not_found(format!("project {}", req.project_id)));
    }
    let report = state
        .env_vault
        .import(&state.store, &req.project_id, Path::new(&req.path))
        .map_err(env_err)?;
    let entries = report
        .entries
        .iter()
        .map(|(key, kind)| EnvEntryInfo { key: key.clone(), kind: kind.clone(), target: None, version: 0, updated_at: None })
        .collect();
    Ok(EnvImportResult {
        imported: report.imported(),
        secrets: report.secrets,
        vars: report.vars,
        skipped: report.skipped,
        entries,
    })
}

// ---- artifact.* (Plan Studio, `plan-studio.md`, `protocol.md` / artifact.*) ----

/// The bounded long-poll budget for `artifact.feedback.poll`: under the ~4-minute
/// harness tool timeout (`agent-cli.md`), returning `pending` so the agent re-polls.
const POLL_BUDGET: Duration = Duration::from_secs(210);
/// Re-check cadence inside a long-poll: the store is truth (feedback is never lost), so a
/// missed wakeup costs at most one tick.
const POLL_TICK: Duration = Duration::from_secs(2);

/// `artifact.register` (root/desktop scope): register or revise an artifact on an
/// explicit card. Used by the desktop and tests; the agent path is `_scoped`.
pub fn artifact_register(state: &AppState, req: ArtifactRegister) -> Result<ArtifactRegistered, ProtocolError> {
    let card_id = req
        .card_id
        .clone()
        .ok_or_else(|| ProtocolError::bad_request("artifact.register needs a card_id"))?;
    register_artifact_inner(state, &card_id, &req.path, req.kind.as_deref(), req.title.as_deref(), &[])
}

/// `artifact.register` (agent scope, `dflow plan open`): the token resolves the card, and
/// the artifact HTML is scanned against the session's known secret values before
/// registration (`security.md`: artifacts are scanned for secrets before registration).
pub fn artifact_register_scoped(
    state: &AppState,
    token: &AgentToken,
    req: ArtifactRegister,
) -> Result<ArtifactRegistered, ProtocolError> {
    let card_id = token
        .card_id
        .clone()
        .ok_or_else(|| ProtocolError::forbidden("this task has no card to open an artifact on"))?;
    let secrets = token
        .session_id()
        .map(|sid| dflow_core::secret::registry().values_for(&sid))
        .unwrap_or_default();
    register_artifact_inner(state, &card_id, &req.path, req.kind.as_deref(), req.title.as_deref(), &secrets)
}

/// Register (or revise) an artifact: read + secret-scan the source HTML, upsert the row,
/// write the served file, and record the open (`artifact_opened` / `plan_round`) with a
/// `plan_round` Needs You item so the human is asked to review.
fn register_artifact_inner(
    state: &AppState,
    card_id: &str,
    path: &str,
    kind: Option<&str>,
    title: Option<&str>,
    session_secrets: &[String],
) -> Result<ArtifactRegistered, ProtocolError> {
    if state.store.get_card(card_id).map_err(store_err)?.is_none() {
        return Err(ProtocolError::not_found(format!("card {card_id}")));
    }
    let src = PathBuf::from(path);
    let html = std::fs::read_to_string(&src)
        .map_err(|e| ProtocolError::bad_request(format!("cannot read artifact file '{path}': {e}")))?;
    // Secret scan before registration (`security.md` / Artifacts): a known secret value
    // in the artifact HTML blocks registration with a finding.
    for secret in session_secrets {
        if !secret.is_empty() && html.contains(secret.as_str()) {
            return Err(ProtocolError::forbidden(
                "artifact contains a known secret value; redact it before opening \
                 (security.md: artifacts are scanned for secrets before registration)",
            ));
        }
    }
    let kind = kind.map(str::trim).filter(|k| !k.is_empty()).unwrap_or("plan");
    let (row, revised) =
        state.store.register_artifact(card_id, path, kind, title).map_err(store_err)?;

    // Write the raw agent HTML under the (stable) doc id; a revision overwrites in place,
    // so the served URL identity is stable and the iframe reloads on the revised nonce.
    let dir = state.data_dir.card_artifacts_dir(card_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ProtocolError::internal(format!("creating artifact dir: {e}")))?;
    let file = state.data_dir.artifact_file(card_id, &row.doc_id);
    std::fs::write(&file, html.as_bytes())
        .map_err(|e| ProtocolError::internal(format!("writing artifact file: {e}")))?;

    let round = row.round.max(0) as u32;
    if revised {
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::PLAN_ROUND,
            serde_json::json!({
                "artifact_id": row.id,
                "round": round,
                "doc_id": row.doc_id,
                "revised_doc_id": row.revised_doc_id,
            }),
        );
    } else {
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::ARTIFACT_OPENED,
            serde_json::json!({ "artifact_id": row.id, "kind": kind, "doc_id": row.doc_id, "round": round }),
        );
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::PLAN_ROUND,
            serde_json::json!({ "artifact_id": row.id, "round": round }),
        );
    }
    // Raise the plan-round Needs You item (the human is asked to review this round).
    let score = needs_input_score(state, Some(card_id));
    let _ = state.store.raise_needs_you(card_id, "plan_round", &plan_round_key(&row.id), score);

    let review_hint = format!(
        "open card {card_id}'s Plan tab in DapperFlow to review artifact {} (round {round}), then run `dflow plan poll`",
        row.id
    );
    Ok(ArtifactRegistered { artifact: row.to_meta(), revised, review_hint })
}

/// The `plan_round` Needs You dedupe key for an artifact.
fn plan_round_key(artifact_id: &str) -> String {
    format!("plan_round:{artifact_id}")
}

/// `artifact.get { artifact_id }` (desktop): metadata + a fresh short-lived signed URL to
/// point the sandboxed iframe at + the latest layout audit.
pub fn artifact_get(state: &AppState, req: ArtifactGet) -> Result<ArtifactGetResult, ProtocolError> {
    let art = state
        .store
        .get_artifact(&req.artifact_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("artifact {}", req.artifact_id)))?;
    let port = state.http_port.load(Ordering::SeqCst);
    let origin = crate::artifact::http_origin(port);
    let (signed_url, expires_at) =
        crate::artifact::signed_url(&state.artifact_signer, &origin, &art.doc_id);
    Ok(ArtifactGetResult {
        artifact: art.to_meta(),
        signed_url,
        expires_at,
        layout_warnings: art.layout_warnings(),
    })
}

/// `artifact.feedback.submit` (desktop / review chrome): store the feedback batch as a
/// queued round, land the layout audit on the artifact, record `feedback_sent` (and
/// `plan_approved` / `artifact_ended` for an approve/end action), resolve the round's
/// Needs You item, and wake any parked `dflow plan poll` (`plan-studio.md`,
/// `phase5-m3-ui.md` / Interpretations 3 and 5).
pub fn feedback_submit(state: &AppState, req: FeedbackSubmit) -> Result<FeedbackSubmitResult, ProtocolError> {
    let art = state
        .store
        .get_artifact(&req.artifact_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("artifact {}", req.artifact_id)))?;
    let round = if req.round > 0 { req.round as i64 } else { art.round };

    // Land the layout audit on the artifact record (flows back through the poll).
    let audit_json = serde_json::to_string(&req.layout_warnings).unwrap_or_else(|_| "[]".to_string());
    state.store.set_artifact_audit(&art.id, &audit_json).map_err(store_err)?;

    let approve = req.items.iter().any(is_approve_action);
    let end = req.items.iter().any(is_end_action);

    if !req.items.is_empty() {
        state.store.add_annotations(&art.id, round, &req.items).map_err(store_err)?;
    }
    let _ = state.store.append_card_event(
        &art.card_id,
        dflow_core::event_kind::FEEDBACK_SENT,
        serde_json::json!({ "artifact_id": art.id, "round": round, "items": req.items.len(), "layout_warnings": req.layout_warnings.len() }),
    );
    // The human responded; the ball returns to the agent.
    let _ = state.store.resolve_needs_you(&art.card_id, &plan_round_key(&art.id), "ui");

    let mut next_step =
        "the agent will pick up your feedback on its next `dflow plan poll`".to_string();
    if approve {
        state
            .store
            .set_artifact_status(&art.id, dflow_core::artifact_status::APPROVED)
            .map_err(store_err)?;
        let _ = state.store.append_card_event(
            &art.card_id,
            dflow_core::event_kind::PLAN_APPROVED,
            serde_json::json!({ "artifact_id": art.id, "round": round }),
        );
        let _ = state.store.append_card_event(
            &art.card_id,
            dflow_core::event_kind::ARTIFACT_ENDED,
            serde_json::json!({ "artifact_id": art.id, "reason": "approved", "round": round }),
        );
        next_step = "plan approved; the agent will stop polling and proceed to implement".to_string();
    } else if end {
        state
            .store
            .set_artifact_status(&art.id, dflow_core::artifact_status::ENDED)
            .map_err(store_err)?;
        let _ = state.store.append_card_event(
            &art.card_id,
            dflow_core::event_kind::ARTIFACT_ENDED,
            serde_json::json!({ "artifact_id": art.id, "reason": "ended", "round": round }),
        );
        next_step = "review ended; the agent will stop polling".to_string();
    }

    // Wake any parked poll so it returns the batch immediately.
    state.artifact_waiters.wake(&art.id);
    Ok(FeedbackSubmitResult { ok: true, round: round.max(0) as u32, revised_doc_id: None, next_step })
}

/// Whether a feedback item is the first-class Approve action (`phase5-m3-ui.md` / 5).
fn is_approve_action(item: &FeedbackItem) -> bool {
    item.kind == "action" && item.action.as_deref() == Some("approve_plan")
}
/// Whether a feedback item ends the review without approval.
fn is_end_action(item: &FeedbackItem) -> bool {
    item.kind == "action" && item.action.as_deref() == Some("end_review")
}

/// Resolve the artifact a poll targets: an explicit id (ownership-checked for an agent),
/// else the token's card's active plan artifact.
fn resolve_poll_artifact(
    state: &AppState,
    token: Option<&AgentToken>,
    artifact_id: Option<&str>,
) -> Result<dflow_core::ArtifactRow, ProtocolError> {
    if let Some(id) = artifact_id.filter(|s| !s.is_empty()) {
        let art = state
            .store
            .get_artifact(id)
            .map_err(store_err)?
            .ok_or_else(|| ProtocolError::not_found(format!("artifact {id}")))?;
        if let Some(t) = token {
            if !t.owns_card(&art.card_id) {
                return Err(ProtocolError::forbidden("artifact is outside this task's scope"));
            }
        }
        return Ok(art);
    }
    let card_id = token
        .and_then(|t| t.card_id.clone())
        .ok_or_else(|| ProtocolError::bad_request("no artifact in scope; pass an artifact_id"))?;
    state
        .store
        .active_plan_artifact(&card_id)
        .map_err(store_err)?
        .ok_or_else(|| {
            ProtocolError::not_found(
                "no plan artifact open for this card; run `dflow plan open <file.html>` first",
            )
        })
}

/// `artifact.feedback.poll` (agent, `dflow plan poll`): the bounded long-poll. Parks the
/// session in `awaiting_feedback` (suspending stuck detection, `adapters.md`) while
/// waiting; returns a queued batch, `ended` + `next_step`, or `pending` + re-poll
/// guidance. Feedback is never lost: an un-consumed batch persists for the next poll.
pub async fn artifact_feedback_poll(
    state: &AppState,
    token: Option<&AgentToken>,
    req: FeedbackPoll,
) -> Result<FeedbackPollResult, ProtocolError> {
    let artifact = resolve_poll_artifact(state, token, req.artifact_id.as_deref())?;
    let artifact_id = artifact.id.clone();
    let session_id = token.and_then(|t| t.session_id());

    // Park the session in awaiting_feedback (suspends tier-3 stuck detection).
    if let Some(sid) = &session_id {
        let _ = state.store.set_session_state(sid, session_state::AWAITING_FEEDBACK);
    }

    let notify = state.artifact_waiters.notify_for(&artifact_id);
    let deadline = Instant::now() + POLL_BUDGET;

    loop {
        let art = state
            .store
            .get_artifact(&artifact_id)
            .map_err(store_err)?
            .ok_or_else(|| ProtocolError::not_found(format!("artifact {artifact_id}")))?;

        // 1. A queued batch is waiting: deliver it (marks it sent exactly once).
        if let Some((round, items)) = state.store.take_queued_batch(&artifact_id).map_err(store_err)? {
            let ended = dflow_core::artifact_status::is_ended(&art.status);
            let approved = art.status == dflow_core::artifact_status::APPROVED;
            // The agent is working again (revising, or proceeding if ended).
            if let Some(sid) = &session_id {
                let _ = state.store.set_session_state(sid, session_state::WORKING);
            }
            return Ok(FeedbackPollResult {
                artifact_id,
                round: round.max(0) as u32,
                items,
                layout_warnings: art.layout_warnings(),
                ended,
                pending: false,
                approved,
                status: art.status.clone(),
                next_step: poll_next_step(ended, approved).to_string(),
            });
        }

        // 2. The review ended with nothing more queued.
        if dflow_core::artifact_status::is_ended(&art.status) {
            if let Some(sid) = &session_id {
                let _ = state.store.set_session_state(sid, session_state::WORKING);
            }
            let approved = art.status == dflow_core::artifact_status::APPROVED;
            return Ok(FeedbackPollResult {
                artifact_id,
                round: art.round.max(0) as u32,
                items: Vec::new(),
                layout_warnings: art.layout_warnings(),
                ended: true,
                pending: false,
                approved,
                status: art.status.clone(),
                next_step: poll_next_step(true, approved).to_string(),
            });
        }

        // 3. Nothing yet. A non-waiting poll (or the deadline) returns pending.
        if !req.wait || Instant::now() >= deadline {
            return Ok(pending_result(&artifact_id, &art));
        }

        // 4. Wait for a wakeup or a tick, bounded by the deadline. A missed wakeup costs
        //    at most one tick because the store is re-read every loop.
        let remaining = deadline.saturating_duration_since(Instant::now());
        let tick = POLL_TICK.min(remaining);
        tokio::select! {
            _ = notify.notified() => {}
            _ = tokio::time::sleep(tick) => {}
        }
    }
}

/// The `pending` poll response: nothing queued yet, re-poll (the session stays
/// `awaiting_feedback`, so stuck detection remains suspended).
fn pending_result(artifact_id: &str, art: &dflow_core::ArtifactRow) -> FeedbackPollResult {
    FeedbackPollResult {
        artifact_id: artifact_id.to_string(),
        round: art.round.max(0) as u32,
        items: Vec::new(),
        layout_warnings: art.layout_warnings(),
        ended: false,
        pending: true,
        approved: false,
        status: art.status.clone(),
        next_step: "no feedback queued yet; run `dflow plan poll` again to keep waiting".to_string(),
    }
}

/// The `next:` line for a delivered/ended poll (`agent-cli.md` design rule 6).
fn poll_next_step(ended: bool, approved: bool) -> &'static str {
    if approved {
        "plan approved; stop polling and proceed to implement in your main channel"
    } else if ended {
        "the review ended; stop polling and proceed per the human's guidance"
    } else {
        "revise the artifact in place, then run `dflow plan open <file>` and `dflow plan poll` again"
    }
}

// ---- service.* (per-project local services, `environments.md`, `data-model.md`) ----

/// `service.add { project_id, name, cmd, scope?, ports?, required? }`: declare (or
/// replace) a local service for a project.
pub fn service_add(state: &AppState, req: ServiceAdd) -> Result<ServiceResult, ProtocolError> {
    if state.store.get_project(&req.project_id).map_err(store_err)?.is_none() {
        return Err(ProtocolError::not_found(format!("project {}", req.project_id)));
    }
    if req.name.trim().is_empty() {
        return Err(ProtocolError::bad_request("service name must not be empty"));
    }
    if req.cmd.trim().is_empty() {
        return Err(ProtocolError::bad_request("service cmd must not be empty"));
    }
    let scope = req.scope.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("per_worktree");
    if !dflow_core::service_scope::is_known(scope) {
        return Err(ProtocolError::bad_request(format!(
            "unknown service scope '{scope}' (per_worktree | shared)"
        )));
    }
    let required = req.required.unwrap_or(true);
    let row = state
        .store
        .set_service(&req.project_id, req.name.trim(), req.cmd.trim(), scope, &req.ports, required)
        .map_err(store_err)?;
    Ok(ServiceResult { service: row.to_info() })
}

/// `service.list { project_id }`.
pub fn service_list(state: &AppState, req: ServiceList) -> Result<ServiceListResult, ProtocolError> {
    let services = state
        .store
        .list_services(&req.project_id)
        .map_err(store_err)?
        .iter()
        .map(|s| s.to_info())
        .collect();
    Ok(ServiceListResult { services })
}

/// `service.remove { project_id, name }`.
pub fn service_remove(state: &AppState, req: ServiceRemove) -> Result<ServiceRemoved, ProtocolError> {
    let removed = state.store.delete_service(&req.project_id, &req.name).map_err(store_err)?;
    Ok(ServiceRemoved { removed })
}

// ---- session.peek and guarded steering (`phase6-mcp.md` merge-time requests) ----

/// `session.peek { session_id, lines? }`: a read-only, bounded, scrubbed plain-text
/// screen capture that never resizes the PTY (`phase6-mcp.md` request 3). Only the
/// daemon can apply the secret scrub, since only it knows the vault values.
pub fn session_peek(state: &AppState, req: SessionPeek) -> Result<SessionPeeked, ProtocolError> {
    let session = state
        .sessions
        .get_str(&req.session_id)
        .ok_or_else(|| ProtocolError::not_found("no such session"))?;
    let max = req.lines.unwrap_or(40).clamp(5, 200) as usize;
    let secrets = dflow_core::secret::registry().values_for(&req.session_id);
    let text = session.peek_scrubbed(max, &secrets);
    let lines = if text.trim().is_empty() { 0 } else { text.lines().count() as u32 };
    Ok(SessionPeeked { session_id: req.session_id, lines, text })
}

/// Resolve a live session for `session.send_verified`, or a caller-facing error. A
/// dead-PTY session refuses before any wire/typing work.
pub fn session_for_send(state: &AppState, session_id: &str) -> Result<Arc<Session>, ProtocolError> {
    let session = state
        .sessions
        .get_str(session_id)
        .ok_or_else(|| ProtocolError::not_found("no such session"))?;
    if !session.is_alive() {
        return Err(ProtocolError::bad_request("session has no live process; it cannot receive input"));
    }
    Ok(session)
}

/// Run a verified submit against `session` (`adapters.md` / Verified submit). Blocking
/// (typing, popup settle, redraw waits), so the caller runs it off the async executor.
/// A harness with no manifest, or whose composer never becomes ready, reports
/// `submitted: false` rather than blindly typing.
pub fn run_send_verified(session: &Session, text: &str) -> SendVerifiedResult {
    let manifest = match bundled_manifests().get(&session.harness) {
        Some(m) => m,
        None => return SendVerifiedResult { submitted: false, attempts: 0 },
    };
    let cfg = SubmitConfig::from_manifest(manifest);
    if !steer::wait_for_composer_ready(session, manifest, Duration::from_secs(8)) {
        return SendVerifiedResult { submitted: false, attempts: 0 };
    }
    let outcome = steer::send_verified(session, manifest, text, &cfg);
    SendVerifiedResult { submitted: outcome.submitted, attempts: outcome.attempts }
}

/// Record a `concertmaster_steered` event on the steered session's card when the caller
/// is mcp-scoped and the text actually reached the composer (`phase6-mcp.md` request 1;
/// `data-model.md` / Concertmaster). The event payload (injected text) is guarded by the
/// same value-matching scrubber as every payload write.
pub fn record_concertmaster_steer(state: &AppState, session_id: &str, text: &str) {
    if let Ok(Some(row)) = state.store.get_session(session_id) {
        if let Some(card_id) = row.card_id {
            let _ = state.store.append_card_event(
                &card_id,
                dflow_core::event_kind::CONCERTMASTER_STEERED,
                serde_json::json!({ "session_id": session_id, "text": text }),
            );
        }
    }
}

/// `auth.mint_concertmaster {}`: mint a Concertmaster-scoped token (owner scope only).
/// The response lists exactly which capability classes the profile withholds.
pub fn mint_concertmaster(state: &AppState) -> Result<ConcertmasterMinted, ProtocolError> {
    let token = state.concertmaster.mint();
    Ok(ConcertmasterMinted { token, excludes: crate::tokens::ConcertmasterRegistry::excludes() })
}

// ---------------------------------------------------------------------------
// M4 Concertmaster rounds (`product.md` / Concertmaster principles: Rounds).
// ---------------------------------------------------------------------------

/// The stable dedupe key for a round's single Needs You digest. Constant per round card
/// (the card is already unique per round type + scope via its origin ref), so a re-run
/// or a second `round.digest` call re-raises the SAME item, never a second.
const ROUND_DIGEST_KEY: &str = "round_digest";

/// The two v1 round types (`product.md` rounds; `knowledge.md` gardener-as-a-round-type).
const ROUND_TYPES: &[&str] = &["floor_check", "garden"];

/// `round.start { round_type, project_id?, agent?, harness? }`: dispatch a headless
/// Concertmaster-scoped round session with a built-in escalation-only brief
/// (`product.md` / Concertmaster rounds). Idempotent on the round card per
/// `(round_type, scope)`, so re-running a round reuses its card and dedupes its digest.
pub fn round_start(state: &AppState, req: RoundStart) -> Result<RoundStarted, ProtocolError> {
    start_round(state, &req.round_type, req.project_id.as_deref(), req.agent.as_deref(), req.harness.as_deref())
}

/// The shared round-dispatch path, called by the `round.start` verb and the scheduler.
pub fn start_round(
    state: &AppState,
    round_type: &str,
    project_id: Option<&str>,
    agent: Option<&str>,
    harness: Option<&str>,
) -> Result<RoundStarted, ProtocolError> {
    let round_type = round_type.trim();
    if !ROUND_TYPES.contains(&round_type) {
        return Err(ProtocolError::bad_request(format!(
            "unknown round type '{round_type}' (expected one of: {})",
            ROUND_TYPES.join(", ")
        )));
    }

    // Scope: a project-scoped round names its project; a global round scopes to "all".
    let (project, scope, cwd) = match project_id {
        Some(pid) => {
            let project = state
                .store
                .get_project(pid)
                .map_err(store_err)?
                .ok_or_else(|| ProtocolError::not_found(format!("project {pid}")))?;
            let cwd = Some(PathBuf::from(&project.path));
            (Some(project), pid.to_string(), cwd)
        }
        None => (None, "all".to_string(), None),
    };

    // The round card anchors the round timeline and its single Needs You digest. Dedupe
    // per (round_type, scope) via the origin ref, so re-running reuses the same card.
    let scope_label = project.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "all projects".into());
    let title = format!("Round: {round_type} ({scope_label})");
    let (round_card, _outcome) = state
        .store
        .upsert_origin_card(NewCard {
            project_id: project.as_ref().map(|p| p.id.clone()),
            card_type: "chore".into(),
            title,
            lane: "performing".into(),
            origin_kind: "concertmaster".into(),
            origin_ref: Some(format!("round:{round_type}:{scope}")),
            ..Default::default()
        })
        .map_err(store_err)?;

    let brief = compose_round_brief(round_type, &scope_label);

    // Launcher resolution: an explicit `agent`/`harness` param wins; else the configured
    // Concertmaster launcher (a project/daemon setting); else a default shell so a round
    // is always dispatchable (the stub path the tests exercise).
    let agent_ref = agent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| state.store.get_setting(setting_key::CONCERTMASTER_LAUNCHER).ok().flatten());
    let harness_ref = harness.map(str::trim).filter(|s| !s.is_empty());
    let (command, harness_name, mut env, agent_id) = if let Some(a) = agent_ref.as_deref() {
        let launch = resolve_agent_launch(state, a)?;
        (launch.command, launch.harness, launch.env, launch.agent_id)
    } else {
        // Launch the harness interactively and submit the brief via first_prompt (like
        // the New Session front door), uniform across a real harness, the default shell,
        // and the `DFLOW_LAUNCH_<H>` test stub. An empty brief here keeps it out of argv.
        let h = harness_ref.unwrap_or("powershell");
        let command = harness::harness_command(h, "", None, None)
            .or_else(|| default_command(h))
            .filter(|c| !c.is_empty())
            .ok_or_else(|| {
                ProtocolError::bad_request(format!(
                    "no launcher for round: unknown harness '{h}' and no agent/concertmaster launcher set"
                ))
            })?;
        (command, h.to_string(), BTreeMap::new(), None)
    };

    // Mint the round token (Concertmaster read surface + `round.digest`, bound to this
    // round card) and inject the agent-CLI env so `dflow` reads state and files the
    // digest. DFLOW_ROUND names the round for `dflow round digest`.
    let (round_token, token_handle) = state.round.mint(&round_card.id, round_type, &scope);
    inject_agent_env(state, &mut env, &round_token, Some(&round_card.id));
    env.insert("DFLOW_ROUND".to_string(), round_card.id.clone());

    let card_ulid = Ulid::from_string(&round_card.id)
        .map_err(|_| ProtocolError::internal("round card id is not a ULID"))?;
    let spec = SessionSpec {
        harness: harness_name.clone(),
        command,
        cols: DISPATCH_COLS,
        rows: DISPATCH_ROWS,
        cwd,
        env,
        card_id: Some(card_ulid),
        project_id: project.as_ref().map(|p| p.id.clone()),
        agent_id,
        title: Some(format!("round: {round_type}")),
        first_prompt: Some(harness::preview(&brief, FIRST_PROMPT_PREVIEW)),
        scrollback_dir: Some(state.data_dir.scrollback_dir()),
        ..Default::default()
    };
    let session = state.sessions.create(spec).map_err(|e| {
        ProtocolError::internal(format!("could not launch round harness '{harness_name}': {e}"))
    })?;
    token_handle.bind_session(&session.id.to_string());

    // Timeline evidence: the round was dispatched (the round card anchors it).
    let _ = state.store.append_card_event(
        &round_card.id,
        event_kind::ROUND_STARTED,
        serde_json::json!({
            "round_type": round_type,
            "scope": scope,
            "session_id": session.id.to_string(),
            "harness": harness_name,
        }),
    );

    // Submit the built-in brief as the first prompt (verified submit once the composer is
    // ready), exactly like the New Session front door.
    spawn_first_prompt_submit(state, Arc::clone(&session), brief);

    Ok(RoundStarted {
        session_id: session.id.to_string(),
        round_card: round_card.id,
        round_type: round_type.to_string(),
        scope,
    })
}

/// The built-in, escalation-only round brief (`product.md`: judgment scope is only what
/// deterministic routing cannot compute; output contract is at most one deduplicated
/// Needs You digest). The garden variant folds in the gardener remit (`knowledge.md`).
fn compose_round_brief(round_type: &str, scope_label: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "You are a headless DapperFlow Concertmaster round ({round_type}) over {scope_label}.\n\n"
    ));
    out.push_str(
        "You run WITHOUT a human watching. Your only output is escalation: at most ONE \
         concise Needs You digest, filed with `dflow round digest --body \"<markdown>\"`. \
         If nothing needs a human, file nothing and exit.\n\n",
    );
    out.push_str("Read the live state first (do not re-derive what routing already computes):\n");
    out.push_str("  - `dflow` fleet + board reads for cross-card synthesis and silence/drift\n");
    out.push_str("  - `dflow know` for the project knowledge digest and catalog\n\n");
    if round_type == "garden" {
        out.push_str(
            "As a knowledge-gardener round: audit recent activity against the knowledgebase. \
             Surface duplicate notes to merge, obviously-missing learnings from completed \
             cards, stale notes (deletion candidates - never delete), and proposed Digest \
             updates. Put these in your one digest as concrete, deep-linked proposals.\n\n",
        );
    } else {
        out.push_str(
            "As a floor-check round: look for cross-card synthesis, silent/stuck work that \
             deterministic thresholds miss, and brief-quality problems. Do NOT duplicate \
             threshold checks the Attention Router already runs.\n\n",
        );
    }
    out.push_str(
        "Rules: escalation-only; ONE digest maximum; no merge/push/dispatch; you have no \
         authority to change work, only to raise what a human should look at.",
    );
    out
}

/// `round.digest { body, findings? }`: file the round's single escalation digest against
/// the token's own round card, and emit `round_completed` (`data-model.md`). Idempotent:
/// a second call updates the same Needs You item in place, so a round can raise AT MOST
/// ONE digest no matter how many times the agent calls it.
pub fn round_digest(
    state: &AppState,
    token: &RoundToken,
    req: RoundDigest,
) -> Result<RoundDigestResult, ProtocolError> {
    let card_id = token.round_card_id.clone();
    let body = req.body.trim();
    if body.is_empty() {
        return Err(ProtocolError::bad_request("`round digest` needs a non-empty --body"));
    }
    let findings = req.findings.unwrap_or(0);

    // Was a digest for this round already open? (the dedupe / re-run signal).
    let deduped = state
        .store
        .list_needs_you(true)
        .map_err(store_err)?
        .iter()
        .any(|i| i.card_id == card_id && i.dedupe_key == ROUND_DIGEST_KEY);

    // Raise (or re-raise) the one digest item. `raise_needs_you` is idempotent on
    // (card_id, dedupe_key), so this can never produce a second item for the round.
    let score = needs_input_score(state, Some(&card_id));
    let item = state
        .store
        .raise_needs_you(&card_id, "round_digest", ROUND_DIGEST_KEY, score)
        .map_err(store_err)?;

    // round_completed carries the findings count + the digest body (scrubbed on write by
    // the secret scrubber in `append_event`), so the round card timeline is the evidence.
    let _ = state.store.append_card_event(
        &card_id,
        event_kind::ROUND_COMPLETED,
        serde_json::json!({
            "round_type": token.round_type,
            "scope": token.scope,
            "findings": findings,
            "digest_item_id": item.id,
            "digest": body,
        }),
    );

    Ok(RoundDigestResult { round_card: card_id, findings, deduped })
}

// ---------------------------------------------------------------------------
// needs_you.* (the attention queue as a first-class list/resolve pair).
// ---------------------------------------------------------------------------

/// `needs_you.list {}`: the open Needs You queue, highest score first.
pub fn needs_you_list(state: &AppState) -> Result<NeedsYouListResult, ProtocolError> {
    let items = state
        .store
        .list_needs_you(true)
        .map_err(store_err)?
        .into_iter()
        .map(|i| NeedsYouItem {
            id: i.id,
            card_id: i.card_id,
            kind: i.kind,
            dedupe_key: i.dedupe_key,
            score: i.score,
            raised_at: i.raised_at,
            notified_at: i.notified_at,
            resolved_at: i.resolved_at,
            resolved_by: i.resolved_by,
        })
        .collect();
    Ok(NeedsYouListResult { items })
}

/// `needs_you.resolve { card_id, dedupe_key }`: resolve one item. `resolved_by` is
/// stamped from the connection scope (`ui` for desktop, `mobile` for a phone client).
pub fn needs_you_resolve(
    state: &AppState,
    req: NeedsYouResolve,
    resolved_by: &str,
) -> Result<NeedsYouResolved, ProtocolError> {
    let resolved = state
        .store
        .resolve_needs_you(&req.card_id, &req.dedupe_key, resolved_by)
        .map_err(store_err)?;
    Ok(NeedsYouResolved { resolved: resolved.is_some() })
}

// ---------------------------------------------------------------------------
// M6 opt-in LAN listener (`security.md` / Remote access trust model).
// ---------------------------------------------------------------------------

/// `daemon.lan.enable { port? }`: bind the LAN listener and persist the toggle + port.
/// `port` omitted reuses the last persisted port, else the default LAN port.
pub async fn lan_enable(state: AppState, req: LanEnable) -> Result<LanState, ProtocolError> {
    let port = req
        .port
        .or_else(|| persisted_lan_port(&state.store))
        .unwrap_or(crate::lan::DEFAULT_LAN_PORT);
    let bound = state
        .lan
        .start(state.clone(), port)
        .await
        .map_err(|e| ProtocolError::internal(format!("could not bind LAN listener on {port}: {e}")))?;
    state.store.set_setting(setting_key::LAN_ENABLED, "1").map_err(store_err)?;
    state.store.set_setting(setting_key::LAN_PORT, &bound.to_string()).map_err(store_err)?;
    lan_status(&state)
}

/// `daemon.lan.disable {}`: stop the LAN listener and persist the toggle off.
pub fn lan_disable(state: &AppState) -> Result<LanState, ProtocolError> {
    state.lan.stop();
    state.store.set_setting(setting_key::LAN_ENABLED, "0").map_err(store_err)?;
    lan_status(state)
}

/// `daemon.lan.status {}`: the listener state, the honest no-TLS caveat, the reachable
/// `/m` URLs, and the live phone pairings for the Settings revocation list.
pub fn lan_status(state: &AppState) -> Result<LanState, ProtocolError> {
    let enabled = state.store.get_bool_setting(setting_key::LAN_ENABLED).map_err(store_err)?;
    let bound_port = state.lan.bound_port();
    let bound = bound_port.is_some();
    let port = bound_port.or_else(|| persisted_lan_port(&state.store)).unwrap_or(0);
    let lan_urls = if bound { crate::lan::lan_urls(port) } else { Vec::new() };
    let phones = state
        .store
        .list_phone_tokens(false)
        .map_err(store_err)?
        .into_iter()
        .map(|p| PhonePairing {
            id: p.id,
            name: p.name,
            created_at: p.created_at,
            last_seen_at: p.last_seen_at,
        })
        .collect();
    Ok(LanState { enabled, bound, port, lan_urls, caveat: crate::lan::LAN_CAVEAT.into(), phones })
}

/// `daemon.lan.pair { name? }`: mint a phone-scoped capability token and build the exact
/// `mobile.md` pairing payload plus a ready-to-encode QR URL. Loopback (owner) scope only
/// (the desktop calls it and renders the QR); a phone token can never reach this verb.
pub fn lan_pair(state: &AppState, req: LanPair) -> Result<LanPairing, ProtocolError> {
    let port = state
        .lan
        .bound_port()
        .or_else(|| persisted_lan_port(&state.store))
        .unwrap_or(crate::lan::DEFAULT_LAN_PORT);
    let ip = crate::lan::primary_lan_ip().ok_or_else(|| {
        ProtocolError::internal("no LAN IP address found; is this machine on a network?")
    })?;

    // Mint the phone token (daemon entropy) and persist it for cross-restart revocation.
    let token = crate::tokens::mint_phone_token();
    let name = req.name.clone().filter(|s| !s.trim().is_empty());
    let token_id = state.store.add_phone_token(&token, name.as_deref()).map_err(store_err)?;

    let payload = PairingPayload {
        url: format!("ws://{ip}:{port}/ws"),
        token,
        name: name.clone(),
    };
    // The fragment (not a query) carries the token, so it never reaches the server in a
    // request line or a server log (`mobile.md` / phase7-pwa.md pairing payload).
    let json = serde_json::to_string(&payload)
        .map_err(|e| ProtocolError::internal(format!("encoding pairing payload: {e}")))?;
    let frag = base64::engine::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        json.as_bytes(),
    );
    let pair_url = format!("http://{ip}:{port}/m#pair={frag}");

    Ok(LanPairing { token_id, pair_url, payload })
}

/// `daemon.lan.revoke { token_id }`: revoke one phone pairing (`security.md` /
/// Per-device revocation). Takes effect at the next handshake; the persisted token is
/// gone, so the device cannot re-authenticate.
pub fn lan_revoke(state: &AppState, req: LanRevoke) -> Result<LanRevoked, ProtocolError> {
    let revoked = state.store.revoke_phone_token(&req.token_id).map_err(store_err)?;
    Ok(LanRevoked { revoked })
}

/// The persisted LAN port setting, parsed, if any.
fn persisted_lan_port(store: &Store) -> Option<u16> {
    store.get_setting(setting_key::LAN_PORT).ok().flatten().and_then(|s| s.parse().ok())
}

/// Rebuild every project's `knowledge_notes` index from disk (daemon start). Only the
/// SQLite index is rebuilt; the on-disk Catalog is regenerated on write, never on start
/// (the daemon must not rewrite the user's repo just by booting).
pub fn rebuild_all_knowledge_indexes(state: &AppState) {
    let projects = match state.store.list_projects() {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!(%err, "could not list projects for knowledge index rebuild");
            return;
        }
    };
    for project in projects {
        if let Ok(Some((dir, _))) = resolve_knowledge_dir(state, &project.id) {
            let notes = knowledge::scan(&dir);
            if let Err(err) = state.store.rebuild_knowledge_index(&project.id, &notes) {
                tracing::debug!(%err, project = %project.id, "knowledge index rebuild failed");
            }
        }
    }
}

/// The `dflow` usage contract injected into every dispatch brief (`adapters.md`
/// dispatch flow step 6; `agent-cli.md` design rules): how to self-report, maintain the
/// board, and consult project memory before re-deriving facts.
const DFLOW_USAGE_CONTRACT: &str = "\
## Working with DapperFlow (the `dflow` CLI)

You are running inside a DapperFlow session. Use the `dflow` CLI to report state and \
maintain the board as a side effect of your work:

- `dflow` shows your current card, state, and next action; `dflow card` shows the brief, \
acceptance criteria, and project memory digest (`--full` for the whole brief).
- Self-report at meaningful boundaries: `dflow status working [note]` when you start or \
change focus, `dflow status blocked \"<why>\"` when you need a human decision (this \
notifies the captain), `dflow status done [note]` when the work is complete.
- Maintain the board: `dflow card note \"<one line>\"` keeps the session strip current; \
`dflow card create --title \"...\" --type <t>` files a follow-up or a bug you tripped over; \
`dflow card move <lane>` moves a card.
- Consult project memory BEFORE re-deriving anything: `dflow know find <query>` searches \
durable notes, `dflow know get <id>` reads one. When you learn something durable, record \
it: `dflow know add --type <t> --title \"...\" --stdin`.";

/// The standing `dflow` guidance injected as ambient context into EVERY session through
/// the harness's system-prompt / first-prompt mechanism, so a plain New Session keeps the
/// board current without the user ever having to say "use dflow" (`agent-cli.md` /
/// Availability and standing guidance; the "standing guidance content" bullets).
///
/// Unlike [`DFLOW_USAGE_CONTRACT`] (composed into the dispatch brief), this is the compact
/// system-prompt form used where there is no brief; it is deliberately lightweight.
pub const DFLOW_STANDING_GUIDANCE: &str = "\
You are running inside a DapperFlow session and the `dflow` CLI is on your PATH. Keep the \
board current as a natural side effect of your work - the user should never have to tell \
you to use it:
- Before re-deriving any project fact, check memory first: `dflow know find <topic>`.
- When you begin real work on something, put it on the board: `dflow card create --title \
\"...\"` (or adopt the card already set for this session if there is one).
- Keep the board honest at meaningful boundaries: `dflow status working \"<short note>\"`, \
`dflow status blocked \"<the decision you need>\"` when you need the human, `dflow status \
done` when finished.
- When you learn something durable (a decision, convention, gotcha, runbook step), record \
it: `dflow know add --type <t> --title \"...\"`.
- Keep it lightweight: a card per real unit of work, not per message. Run `dflow` any time \
to see your current card, state, and next step.";

/// How a harness received (or could not receive) the standing guidance for a session,
/// per its manifest `context_injection` method (`adapters.md` / Standing-guidance
/// injection). Returned by [`apply_standing_guidance`] so the caller knows whether it must
/// still prepend a first-prompt preamble.
pub enum GuidanceInjection {
    /// The system-prompt flag was spliced into the launch argv; nothing else to do.
    SystemPrompt,
    /// No system-prompt flag; the caller must prepend this preamble to the session's
    /// first prompt (degraded, but non-polluting). Carries the guidance text.
    FirstPromptPreamble(String),
    /// No non-polluting mechanism (or no manifest / a plain shell): the session launches
    /// without standing guidance rather than writing into the user's checkout.
    None,
}

/// Inject the standing `dflow` guidance into a session launch the least-intrusive way the
/// harness allows, mutating `command` in place for the `append_system_prompt` method
/// (`adapters.md` / Standing-guidance injection). Returns how it was handled so the caller
/// can complete a first-prompt fallback.
///
/// This is the New Session path: dispatch composes the contract into the brief already,
/// and a Concertmaster round carries its own purpose-built brief, so this is applied only
/// where a session would otherwise have no ambient dflow guidance at all.
pub fn apply_standing_guidance(harness: &str, command: &mut Vec<String>) -> GuidanceInjection {
    let manifest = match bundled_manifests().get(harness) {
        Some(m) => m,
        // A plain shell (powershell/cmd) or an unmanifested command: nothing to inject.
        None => return GuidanceInjection::None,
    };
    match manifest.context_injection_method() {
        dflow_core::manifest::CI_APPEND_SYSTEM_PROMPT => {
            if let Some(flag) = manifest.context_injection_flag(DFLOW_STANDING_GUIDANCE) {
                // Splice the flag right after the command binary (argv[0]), so it groups
                // with the manifest flags and never lands after a trailing positional or a
                // launcher's extra_args.
                let insert_at = command.len().min(1);
                for (i, tok) in flag.into_iter().enumerate() {
                    command.insert(insert_at + i, tok);
                }
                GuidanceInjection::SystemPrompt
            } else {
                GuidanceInjection::None
            }
        }
        dflow_core::manifest::CI_FIRST_PROMPT => {
            GuidanceInjection::FirstPromptPreamble(DFLOW_STANDING_GUIDANCE.to_string())
        }
        // CI_NONE (or anything else): flagged guidance-unsupported for New Session.
        _ => GuidanceInjection::None,
    }
}

// ---- helpers ----

/// Compose the dispatch brief (`adapters.md` dispatch flow step 6): the card brief, its
/// acceptance criteria, the project knowledge digest (30-line capped), the recipe's
/// stage-tagged guidance, and the `dflow` usage contract, so any harness gets the full
/// agent-side contract.
fn compose_dispatch_brief(
    state: &AppState,
    card: &dflow_proto::Card,
    project: &dflow_proto::Project,
    recipe: &Recipe,
) -> String {
    let mut out = harness::compose_brief(&card.title, card.brief.as_deref());

    let acceptance = card.brief.as_deref().map(parse_acceptance).unwrap_or_default();
    if !acceptance.is_empty() {
        out.push_str("\n\n## Acceptance criteria\n");
        for (i, item) in acceptance.iter().enumerate() {
            out.push_str(&format!("{}. {}\n", i + 1, item));
        }
    }

    let over = state.store.project_knowledge_path(&project.id).ok().flatten();
    let dir = knowledge::resolve_dir(Path::new(&project.path), over.as_deref());
    if let Some(digest) = knowledge::read_digest(&dir) {
        out.push_str("\n\n## Project memory digest\n");
        out.push_str(digest.trim_end());
        out.push('\n');
    }

    out.push_str("\n\n");
    out.push_str(&render_recipe_brief_section(recipe));

    out.push_str("\n\n");
    out.push_str(DFLOW_USAGE_CONTRACT);
    out
}

/// Render the recipe's brief section: identity, stage list, engine-enforced budgets,
/// the plan-approval honesty note, and the stage-tagged guidance from the recipe body
/// (`recipes.md` / Format: "the body is stage-tagged natural-language guidance injected
/// into agent briefs").
fn render_recipe_brief_section(recipe: &Recipe) -> String {
    let mut out = format!("## Flow recipe: {} v{}\n", recipe.name, recipe.version);
    if let Some(desc) = &recipe.description {
        out.push_str(desc.trim());
        out.push('\n');
    }
    let stages: Vec<&str> = recipe.stages.iter().map(|s| s.as_str()).collect();
    out.push_str(&format!("Stages: {}.\n", stages.join(" -> ")));

    if let Some(budgets) = &recipe.budgets {
        let mut parts = Vec::new();
        if let Some(cards) = budgets.cards {
            parts.push(format!("{cards} cards"));
        }
        if let Some(notes) = budgets.notes {
            parts.push(format!("{notes} notes"));
        }
        if !parts.is_empty() {
            out.push_str(&format!(
                "Budgets (engine-enforced): you may create at most {} in this session; past a cap \
                 the CLI returns a structured error - rank the remainder into your report instead.\n",
                parts.join(" and ")
            ));
        }
    }

    // Honesty note (`recipes.md` bundled deep recipe): plan approval is recipe policy
    // now, but the daemon cannot hold a session at an unapproved plan until Plan
    // Studio lands, so the brief says so instead of implying an enforced gate.
    if recipe.has_stage(Stage::Plan)
        && recipe.plan.as_ref().is_some_and(|p| p.approval == dflow_core::recipe::Approval::Required)
    {
        out.push_str(
            "Note: this recipe requires plan approval, but daemon-enforced plan gating arrives \
             with Plan Studio; until then keep the human in the loop through status notes and do \
             not treat `dflow status done` as plan approval.\n",
        );
    }

    for guidance in &recipe.guidance {
        // Inject only stages the recipe actually runs; inherited or leftover sections
        // for absent stages would be noise.
        if !recipe.has_stage(guidance.stage) {
            continue;
        }
        out.push_str(&format!("\n### {} guidance\n", guidance.stage.as_str()));
        out.push_str(guidance.text.trim_end());
        out.push('\n');
    }
    out
}

/// Parse acceptance criteria from a card brief: the list items under a heading or label
/// matching "acceptance" (case-insensitive), until the next heading or a blank gap.
/// Deterministic and permissive; an empty result is a definitive "none recorded".
fn parse_acceptance(brief: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in brief.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        let is_heading = trimmed.starts_with('#');
        let is_label = lower.starts_with("acceptance");
        if is_heading || is_label {
            // Enter the section on an "acceptance" heading/label; leave on any other heading.
            in_section = lower.contains("acceptance");
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty() {
            if !items.is_empty() {
                break; // a blank line after items ends the section
            }
            continue;
        }
        if let Some(rest) = strip_list_marker(trimmed) {
            if !rest.is_empty() {
                items.push(rest.to_string());
            }
        } else {
            // A non-list line ends the section (prose follows).
            break;
        }
    }
    items
}

/// Strip a leading list marker (`- `, `* `, `N. `, `N) `) from a line, returning the
/// remainder, or `None` if the line is not a list item.
fn strip_list_marker(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(rest.trim());
    }
    // Numbered forms: "1. text" or "1) text".
    let mut chars = line.char_indices();
    let mut saw_digit = false;
    for (idx, ch) in chars.by_ref() {
        if ch.is_ascii_digit() {
            saw_digit = true;
            continue;
        }
        if saw_digit && (ch == '.' || ch == ')') {
            let rest = line[idx + ch.len_utf8()..].trim();
            return Some(rest);
        }
        break;
    }
    None
}

/// Detect a repo's default branch: the current branch if any, else the remote HEAD,
/// else `main` (a detached fresh clone gives git nothing better to report).
fn detect_default_branch(repo: &Path) -> String {
    if let Ok(branch) = git_capture(repo, &["symbolic-ref", "--short", "HEAD"]) {
        let branch = branch.trim();
        if !branch.is_empty() {
            return branch.to_string();
        }
    }
    if let Ok(head) = git_capture(repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        if let Some(name) = head.trim().rsplit('/').next() {
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    "main".to_string()
}

/// Run a git subcommand in `cwd`, returning trimmed stdout or stderr as the error.
fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("git not runnable: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Map a store error onto the wire error taxonomy.
pub fn store_err(err: StoreError) -> ProtocolError {
    match err {
        StoreError::NotFound(what) => ProtocolError::not_found(what),
        StoreError::Invalid(what) => ProtocolError::bad_request(what),
        other => ProtocolError::internal(other.to_string()),
    }
}

/// Map a worktree pool error onto the wire error taxonomy.
pub fn wt_err(err: WorktreeError) -> ProtocolError {
    match err {
        WorktreeError::NotFound(what) => ProtocolError::not_found(what),
        WorktreeError::Store(e) => store_err(e),
        other => ProtocolError::internal(other.to_string()),
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dflow_core::recipe::RecipeCatalog;
    use dflow_core::{DataDir, NewSession};

    #[test]
    fn standing_guidance_splices_claude_system_prompt_flag() {
        // claude uses the append_system_prompt method, so the flag is spliced into argv
        // right after the binary, carrying the standing guidance - no repo pollution.
        let mut cmd = vec!["claude".to_string(), "--permission-mode".to_string(), "acceptEdits".to_string()];
        let result = apply_standing_guidance("claude", &mut cmd);
        assert!(matches!(result, GuidanceInjection::SystemPrompt));
        assert_eq!(cmd[0], "claude");
        assert_eq!(cmd[1], "--append-system-prompt");
        assert_eq!(cmd[2], DFLOW_STANDING_GUIDANCE);
        // The original flags are preserved after the injected pair.
        assert!(cmd.windows(2).any(|w| w == ["--permission-mode", "acceptEdits"]));
    }

    #[test]
    fn standing_guidance_fallback_is_a_first_prompt_preamble() {
        // codex has no system-prompt flag, so the caller is handed the preamble to prepend
        // to the first prompt (degraded, but never into the user's checkout). The argv is
        // left untouched.
        let mut cmd = vec!["codex".to_string()];
        match apply_standing_guidance("codex", &mut cmd) {
            GuidanceInjection::FirstPromptPreamble(text) => assert_eq!(text, DFLOW_STANDING_GUIDANCE),
            _ => panic!("codex should hand back a first-prompt preamble"),
        }
        assert_eq!(cmd, vec!["codex".to_string()], "first_prompt fallback never edits argv");
    }

    #[test]
    fn standing_guidance_none_for_unsupported_or_plain_shell() {
        // cursor is flagged guidance-unsupported; a plain shell has no manifest. Both leave
        // the argv untouched and inject nothing.
        let mut cursor = vec!["cursor-agent".to_string()];
        assert!(matches!(apply_standing_guidance("cursor", &mut cursor), GuidanceInjection::None));
        assert_eq!(cursor, vec!["cursor-agent".to_string()]);
        let mut shell = vec!["powershell".to_string()];
        assert!(matches!(apply_standing_guidance("powershell", &mut shell), GuidanceInjection::None));
    }

    #[test]
    fn standing_guidance_text_carries_the_contract_bullets() {
        // The injected guidance must actually tell the agent when and how to use dflow
        // (agent-cli.md / standing guidance content), so a New Session is self-explaining.
        let g = DFLOW_STANDING_GUIDANCE;
        assert!(g.contains("dflow know find"), "must say to consult memory first");
        assert!(g.contains("dflow card create"), "must say to put work on the board");
        assert!(g.contains("dflow status working"), "must say to self-report progress");
        assert!(g.contains("dflow status blocked"), "must say to escalate when blocked");
        assert!(g.contains("dflow status done"), "must say to report completion");
        assert!(g.contains("dflow know add"), "must say to record durable learnings");
    }

    /// A minimal in-memory `AppState` for handler unit tests (no listener, no PTYs).
    fn test_state() -> (AppState, tempdir_guard::TempDir) {
        let tmp = tempdir_guard::TempDir::new("dflowd-api");
        let store = std::sync::Arc::new(dflow_core::Store::open_in_memory().unwrap());
        let sessions = std::sync::Arc::new(dflow_core::SessionManager::with_store(std::sync::Arc::clone(&store)));
        let worktrees = std::sync::Arc::new(dflow_core::WorktreePool::new(
            std::sync::Arc::clone(&store),
            tmp.path().join("worktrees"),
        ));
        let state = AppState {
            sessions,
            store,
            worktrees,
            data_dir: std::sync::Arc::new(DataDir::at(tmp.path())),
            token: std::sync::Arc::new("root-test-token".to_string()),
            daemon_version: std::sync::Arc::new("test".to_string()),
            shutdown: std::sync::Arc::new(tokio::sync::Notify::new()),
            hooks: std::sync::Arc::new(crate::hooks::HookRegistry::default()),
            tokens: std::sync::Arc::new(crate::tokens::TokenRegistry::default()),
            env_vault: std::sync::Arc::new(dflow_core::EnvVault::new()),
            concertmaster: std::sync::Arc::new(crate::tokens::ConcertmasterRegistry::default()),
            round: std::sync::Arc::new(crate::tokens::RoundRegistry::default()),
            lan: std::sync::Arc::new(crate::lan::LanListener::default()),
            http_port: std::sync::Arc::new(std::sync::atomic::AtomicU16::new(0)),
            artifact_signer: std::sync::Arc::new(crate::artifact::ArtifactSigner::new()),
            artifact_waiters: std::sync::Arc::new(crate::artifact::ArtifactWaiters::default()),
            services: std::sync::Arc::new(dflow_core::ServiceManager::new()),
        };
        (state, tmp)
    }

    /// A tiny self-cleaning temp dir so the state test leaves nothing behind.
    mod tempdir_guard {
        use std::path::{Path, PathBuf};
        use std::time::{SystemTime, UNIX_EPOCH};
        pub struct TempDir(PathBuf);
        impl TempDir {
            pub fn new(tag: &str) -> Self {
                let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
                let dir = std::env::temp_dir().join(format!("{tag}-{n}"));
                std::fs::create_dir_all(&dir).unwrap();
                TempDir(dir)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    /// `record_concertmaster_steer` appends a `concertmaster_steered` event carrying the
    /// injected text and session id on the steered session's card (`phase6-mcp.md`
    /// merge-time request 1). This is the attribution the mcp/Concertmaster scope emits.
    #[test]
    fn concertmaster_steer_records_attributed_event() {
        let (state, _tmp) = test_state();
        let project = state.store.add_project("/tmp/steer", "steer", "main", "pr").unwrap();
        let card = state
            .store
            .create_card(dflow_core::NewCard {
                project_id: Some(project.id.clone()),
                title: "Steer me".into(),
                ..Default::default()
            })
            .unwrap();
        let session_id = ulid::Ulid::new().to_string();
        state
            .store
            .create_session(NewSession {
                id: session_id.clone(),
                card_id: Some(card.id.clone()),
                project_id: Some(project.id.clone()),
                cwd: None,
                harness: "claude".into(),
                model: None,
                effort: None,
                state: "working".into(),
                worktree_id: None,
                scrollback_path: format!("{session_id}.ring"),
                first_prompt: None,
                resumed_from: None,
                title: None,
                agent_id: None,
            })
            .unwrap();

        record_concertmaster_steer(&state, &session_id, "get unstuck: run the failing test");

        let events = state.store.card_events(&card.id, None, 50).unwrap();
        let steered = events
            .iter()
            .find(|e| e.kind == dflow_core::event_kind::CONCERTMASTER_STEERED)
            .expect("a concertmaster_steered event must be recorded");
        assert_eq!(steered.payload["session_id"].as_str(), Some(session_id.as_str()));
        assert_eq!(
            steered.payload["text"].as_str(),
            Some("get unstuck: run the failing test"),
            "the event carries the injected text as attribution"
        );
    }

    /// `mint_concertmaster` registers a token and reports the withheld capability classes
    /// (`security.md` / Concertmaster capability scope).
    #[test]
    fn mint_concertmaster_reports_exclusions() {
        let (state, _tmp) = test_state();
        assert_eq!(state.concertmaster.len(), 0);
        let minted = mint_concertmaster(&state).unwrap();
        assert!(state.concertmaster.contains(&minted.token), "the minted token is registered");
        assert_eq!(state.concertmaster.len(), 1);
        assert!(minted.excludes.iter().any(|e| e.contains("vault")), "vault is excluded: {:?}", minted.excludes);
        assert!(minted.excludes.iter().any(|e| e.contains("kill")), "kill is excluded");
    }

    /// The recipe brief section carries identity, stages, budgets, the plan-approval
    /// honesty note, and the stage-tagged guidance (`recipes.md` / Format).
    #[test]
    fn recipe_brief_section_injects_stage_guidance() {
        let catalog = RecipeCatalog::build(None, None);
        let standard = catalog.resolve("standard").unwrap().recipe;
        let out = render_recipe_brief_section(&standard);
        assert!(out.starts_with("## Flow recipe: standard v1\n"), "got: {out}");
        assert!(out.contains("Stages: plan -> implement -> verify -> ship.\n"));
        // Plan approval is required but not yet daemon-enforced; the brief says so.
        assert!(out.contains("daemon-enforced plan gating arrives"), "honesty note missing: {out}");
        // Stage-tagged guidance from the recipe body is injected under stage headings.
        assert!(out.contains("### plan guidance\n"));
        assert!(out.contains("Keep the artifact to one screen."));
        assert!(out.contains("### implement guidance\n"));
        // No budgets on standard, so no budget line.
        assert!(!out.contains("Budgets (engine-enforced)"));
    }

    #[test]
    fn recipe_brief_section_carries_budgets_for_audit() {
        let catalog = RecipeCatalog::build(None, None);
        let audit = catalog.resolve("audit").unwrap().recipe;
        let out = render_recipe_brief_section(&audit);
        assert!(out.contains("## Flow recipe: audit v1\n"));
        assert!(out.contains("at most 10 cards and 6 notes"), "budget line missing: {out}");
        assert!(out.contains("### implement guidance\n"));
        assert!(out.contains("scout, not a fixer"));
        // No plan stage, so no plan-approval note.
        assert!(!out.contains("plan gating"));
    }

    /// `done` arbitration answers with the recipe's remaining stages (`agent-cli.md`).
    #[test]
    fn recipe_done_next_reflects_stage_list() {
        let catalog = RecipeCatalog::build(None, None);
        let standard = catalog.resolve("standard").unwrap().recipe;
        let next = recipe_done_next(&standard);
        assert!(
            next.contains("verify (checks_only) then ship (pr)"),
            "standard next should name the remaining stages: {next}"
        );
        let audit = catalog.resolve("audit").unwrap().recipe;
        let next = recipe_done_next(&audit);
        assert!(next.contains("ends at implement"), "audit next: {next}");
    }
}
