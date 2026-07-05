//! The loopback WebSocket server, shared state, and graceful shutdown.
//!
//! Phase 1: the daemon opens the SQLite store at startup, reconciles interrupted
//! sessions (`architecture.md` / daemon restarts), owns the worktree pool, and runs
//! the needs-input supervision loop (`roadmap.md` M1, lifecycle v0).

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use dflow_core::{
    heuristics, secret, session_state, DataDir, EnvVault, ServiceManager, SessionManager, Store,
    WorktreePool,
};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use crate::artifact::{
    artifact_asset_handler, artifact_doc_handler, ArtifactSigner, ArtifactWaiters,
};
use crate::conn::handle_socket;
use crate::hooks::{hook_handler, HookRegistry};
use crate::lan::LanListener;
use crate::runtime::{Runtime, RuntimeInfo};
use crate::tokens::{ConcertmasterRegistry, RoundRegistry, TokenRegistry};

/// How often the supervisor samples dispatch-session screens.
const SUPERVISOR_TICK: Duration = Duration::from_millis(750);

/// Shared daemon state handed to every connection.
#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionManager>,
    pub store: Arc<Store>,
    pub worktrees: Arc<WorktreePool>,
    pub data_dir: Arc<DataDir>,
    pub token: Arc<String>,
    pub daemon_version: Arc<String>,
    pub shutdown: Arc<Notify>,
    /// Tier-2 hook token registry (`token -> session id`) for the loopback endpoint.
    pub hooks: Arc<HookRegistry>,
    /// Per-task token registry (`security.md` / Per-task tokens) for the `dflow` CLI.
    pub tokens: Arc<TokenRegistry>,
    /// The per-project env vault (`environments.md`), owning the OS credential-store
    /// backend used to seal/unseal and materialize vault values.
    pub env_vault: Arc<EnvVault>,
    /// Concertmaster-scoped token registry (`security.md` / Concertmaster capability
    /// scope), minted via `auth.mint_concertmaster`.
    pub concertmaster: Arc<ConcertmasterRegistry>,
    /// Round-scoped token registry (`product.md` / Concertmaster rounds; M4), minted at
    /// `round.start` and bound to the round's headless session.
    pub round: Arc<RoundRegistry>,
    /// The opt-in LAN listener (`security.md` / Remote access trust model; M6). Off by
    /// default; `daemon.lan.enable` binds a second 0.0.0.0 listener serving the PWA + WS.
    pub lan: Arc<LanListener>,
    /// The loopback HTTP port, set once the listener binds (0 until then). Used to
    /// build the per-session hook URL wired into claude's `--settings`, the `dflow`
    /// CLI's `DFLOW_ENDPOINT`, and the codex notify bridge.
    pub http_port: Arc<AtomicU16>,
    /// Signs/verifies short-lived artifact URLs (`security.md` / Artifact sandbox).
    pub artifact_signer: Arc<ArtifactSigner>,
    /// Per-artifact long-poll waiters for `artifact.feedback.poll`.
    pub artifact_waiters: Arc<ArtifactWaiters>,
    /// Running per-worktree services + the port broker (`environments.md`).
    pub services: Arc<ServiceManager>,
}

/// Bind a loopback listener, publish the runtime file, and serve until shutdown.
pub async fn run(runtime: Runtime, version: String) -> Result<()> {
    let data_dir = Arc::new(runtime.data_dir().clone());
    let store = Arc::new(Store::open(&data_dir.db_file()).context("opening store")?);

    // Startup reconciliation: any session row still marked live belonged to a
    // previous daemon process, and its PTY died with it (children are tied to the
    // daemon's job object). Mark them interrupted, never delete (`architecture.md`).
    let interrupted = store.reconcile_interrupted().context("reconciling sessions")?;
    if !interrupted.is_empty() {
        tracing::info!(count = interrupted.len(), "marked stale sessions interrupted");
    }

    let sessions = Arc::new(SessionManager::with_store(Arc::clone(&store)));
    let worktrees = Arc::new(WorktreePool::new(Arc::clone(&store), data_dir.worktrees_dir()));
    let shutdown = Arc::new(Notify::new());
    let http_port = Arc::new(AtomicU16::new(0));
    let state = AppState {
        sessions: Arc::clone(&sessions),
        store: Arc::clone(&store),
        worktrees,
        data_dir,
        token: Arc::new(runtime.token().to_string()),
        daemon_version: Arc::new(version.clone()),
        shutdown: Arc::clone(&shutdown),
        hooks: Arc::new(HookRegistry::default()),
        tokens: Arc::new(TokenRegistry::default()),
        env_vault: Arc::new(EnvVault::new()),
        concertmaster: Arc::new(ConcertmasterRegistry::default()),
        round: Arc::new(RoundRegistry::default()),
        lan: Arc::new(LanListener::default()),
        http_port: Arc::clone(&http_port),
        artifact_signer: Arc::new(ArtifactSigner::new()),
        artifact_waiters: Arc::new(ArtifactWaiters::default()),
        services: Arc::new(ServiceManager::new()),
    };
    if !state.env_vault.crypto_is_secure() {
        tracing::warn!(
            "env vault is using the INSECURE non-Windows developer crypto stub; \
             vault values are obfuscated, not encrypted (see dflow_core::env::crypto)"
        );
    }

    // Loopback only, OS-assigned port (`protocol.md` / Transport).
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding loopback listener")?;
    let port = listener.local_addr()?.port();
    // Publish the port so the tier-2 hook endpoint can build its per-session URL.
    http_port.store(port, Ordering::SeqCst);
    let info: RuntimeInfo = runtime.publish(port)?;
    tracing::info!(port = info.port, pid = info.pid, "dflowd listening on loopback");
    // A clear line on stdout so a supervising parent (the Tauri shell) can confirm
    // readiness without parsing the runtime file.
    println!("dflowd ready port={} pid={}", info.port, info.pid);

    // Rebuild the knowledge_notes index from disk for every project (`knowledge.md`:
    // "rebuilt from the directory on daemon start"). The on-disk Catalog is left alone;
    // it regenerates only on write, so a daemon boot never rewrites the user's repo.
    crate::api::rebuild_all_knowledge_indexes(&state);

    // Rebuild the recipes index from disk (`data-model.md`: file is truth, the DB row
    // is an index, rebuilt on start and on install/change).
    crate::recipes::rebuild_recipe_index(&state);

    // If the opt-in LAN listener was left enabled, rebind it now (`security.md` /
    // Remote access: the toggle is persisted, so a paired phone reconnects after a
    // daemon restart without the desktop re-enabling it). A bind failure is logged, not
    // fatal: loopback stays up regardless.
    if state.store.get_bool_setting(dflow_core::setting_key::LAN_ENABLED).unwrap_or(false) {
        let port = state
            .store
            .get_setting(dflow_core::setting_key::LAN_PORT)
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .unwrap_or(crate::lan::DEFAULT_LAN_PORT);
        if let Err(err) = state.lan.start(state.clone(), port).await {
            tracing::warn!(%err, port, "could not rebind the persisted LAN listener at startup");
        }
    }

    // The tier-3 supervision loop (needs-input v0).
    let supervisor = tokio::spawn(run_supervisor(state.clone()));
    // The round scheduler (`product.md` / Concertmaster rounds: scheduled runs, off by
    // default). Cheap when no project schedules anything.
    let scheduler = tokio::spawn(run_round_scheduler(state.clone()));

    // Keep a handle to the running services so shutdown reaps them (their process trees
    // die with the Job Object regardless, this is the clean stop).
    let services = Arc::clone(&state.services);
    // Keep a handle to the LAN listener so shutdown stops its serve task cleanly.
    let lan = Arc::clone(&state.lan);

    let app = Router::new()
        .route("/health", get(health))
        .route("/ws", get(ws_handler))
        // Tier-2 native signals: claude POSTs lifecycle hooks here (`adapters.md`).
        .route("/hooks/{token}", post(hook_handler))
        // The loopback artifact service (`security.md` / Artifact sandbox): a signed
        // document endpoint and the same-origin injected SDK/mermaid assets.
        .route("/artifact/doc/{doc_id}", get(artifact_doc_handler))
        .route("/artifact/asset/{name}", get(artifact_asset_handler))
        // Graceful shutdown for `dflowd --stop` (root bearer token required).
        .route("/shutdown", post(shutdown_handler))
        // `dflowd --pair`: enable the LAN listener (if needed) and mint a phone pairing,
        // returning the QR URL + payload. Loopback + root bearer token only.
        .route("/pair", post(pair_handler))
        .with_state(state);

    // Trigger graceful shutdown on either the in-protocol daemon.shutdown verb or
    // Ctrl-C. Sessions are killed on the way out.
    let shutdown_signal = {
        let shutdown = Arc::clone(&shutdown);
        async move {
            tokio::select! {
                _ = shutdown.notified() => tracing::info!("shutdown requested via protocol"),
                _ = tokio::signal::ctrl_c() => tracing::info!("shutdown requested via ctrl-c"),
            }
        }
    };

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await;

    tracing::info!("daemon shutting down; marking sessions interrupted and killing their trees");
    supervisor.abort();
    scheduler.abort();
    // Stop the LAN listener (its serve task also ends with the process/Job Object).
    lan.stop();
    // Stop every per-worktree service (their trees die with the Job Object too).
    services.stop_all();
    // Graceful shutdown marks sessions interrupted (resumable) and takes every process
    // tree with it via the Job Object, so no agent CLI is ever orphaned.
    sessions.shutdown_all_interrupted();
    runtime.cleanup();
    serve_result.context("serving")?;
    Ok(())
}

/// The needs-input supervision loop (`adapters.md` tier 3, deliverable 9).
///
/// Every tick, classify each dispatch session's visible screen. An idle composer at
/// a permission/trust prompt transitions the session to `needs_input` (appending the
/// event and raising a Needs You item, atomically in the store); the prompt clearing
/// resolves it back to `working`. Sessions in terminal states are left alone.
async fn run_supervisor(state: AppState) {
    let mut tick = tokio::time::interval(SUPERVISOR_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        // Supervise every live persisted session (carded and cardless). A cardless
        // session still transitions state; it just raises no card-scoped Needs You.
        for session in state.sessions.live_sessions() {
            if !session.is_alive() {
                // Prune a finished session's hook and per-task tokens so both registries
                // stay bounded and a leaked token cannot outlive its session. Also drop
                // its materialized secrets from the scrubber registry (their materialized
                // files are shredded at worktree return; this just bounds memory).
                let sid = session.id.to_string();
                state.hooks.unregister_session(&sid);
                state.tokens.revoke_session(&sid);
                state.round.revoke_session(&sid);
                secret::registry().unregister(&sid);
                continue;
            }
            let id = session.id.to_string();
            let row = match state.store.get_session(&id) {
                Ok(Some(row)) => row,
                Ok(None) => continue,
                Err(err) => {
                    tracing::debug!(%err, session_id = %id, "supervisor could not read session row");
                    continue;
                }
            };
            if session_state::is_terminal(&row.state) {
                continue;
            }
            // A session parked in awaiting_feedback (plan review) is legitimately idle;
            // tier-3 stuck detection is suspended for it (`architecture.md`).
            if row.state == session_state::AWAITING_FEEDBACK {
                continue;
            }
            let screen = session.capture_plain();
            let wants_input = heuristics::needs_input(&session.harness, &screen);
            let busy = heuristics::is_busy(&session.harness, &screen);
            // Distinguish a trust/permission gate from an agent blocked awaiting a
            // decision, so Needs You carries the right kind (Phase 2, deliverable 7).
            let kind = if heuristics::is_trust_dialog(&session.harness, &screen) {
                "trust_dialog"
            } else {
                "agent_blocked"
            };
            let result = if wants_input && row.state != session_state::NEEDS_INPUT {
                let score = crate::api::needs_input_score(&state, row.card_id.as_deref());
                state.store.mark_session_needs_input(&id, kind, score)
            } else if row.state == session_state::NEEDS_INPUT && busy {
                // Only clear needs_input once the agent is actively working again, so a
                // tier-3 pass never prematurely clears a tier-2 (hook) needs_input just
                // because a prompt is not currently on screen (`adapters.md` /
                // Disagreement policy).
                state.store.clear_session_needs_input(&id, session_state::WORKING)
            } else {
                Ok(())
            };
            if let Err(err) = result {
                tracing::debug!(%err, session_id = %id, "supervisor state transition failed");
            }
        }
    }
}

/// The round scheduler (`product.md` / Concertmaster rounds: scheduled or user-triggered
/// runs, off by default, per-project schedulable). Every tick it reads the projects that
/// carry a `rounds_schedule` / `gardener_schedule` (all others are skipped by one cheap
/// query), and for each enabled+due schedule it dispatches the same `start_round` path a
/// user-triggered round takes, then bookmarks the run so the interval is respected.
///
/// The tick interval defaults to 60s; `DFLOW_ROUND_TICK_MS` overrides it (tests use a
/// short tick). A schedule is `{"enabled": bool, "interval_minutes": u64}` json.
async fn run_round_scheduler(state: AppState) {
    let tick_ms: u64 = std::env::var("DFLOW_ROUND_TICK_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(60_000);
    let mut tick = tokio::time::interval(Duration::from_millis(tick_ms));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let schedules = match state.store.list_project_schedules() {
            Ok(s) => s,
            Err(err) => {
                tracing::debug!(%err, "round scheduler could not read project schedules");
                continue;
            }
        };
        for (project_id, rounds, gardener) in schedules {
            maybe_run_scheduled_round(&state, &project_id, "floor_check", rounds.as_deref());
            maybe_run_scheduled_round(&state, &project_id, "garden", gardener.as_deref());
        }
    }
}

/// Dispatch one scheduled round if its schedule is enabled and its interval has elapsed
/// since the last run (bookmarked in `settings`). A malformed or disabled schedule is a
/// no-op.
fn maybe_run_scheduled_round(
    state: &AppState,
    project_id: &str,
    round_type: &str,
    schedule_json: Option<&str>,
) {
    let interval_minutes = match parse_schedule_interval(schedule_json) {
        Some(m) => m,
        None => return, // absent, malformed, or disabled
    };
    let bookmark = format!("round.last_run:{round_type}:{project_id}");
    let last_run: i64 = state
        .store
        .get_setting(&bookmark)
        .ok()
        .flatten()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let now = now_ms();
    if now - last_run < (interval_minutes as i64) * 60_000 {
        return; // not due yet
    }
    // Bookmark first so a slow/failed dispatch cannot cause a tight re-dispatch loop.
    let _ = state.store.set_setting(&bookmark, &now.to_string());
    match crate::api::start_round(state, round_type, Some(project_id), None, None) {
        Ok(started) => {
            tracing::info!(
                round_type,
                project_id,
                session_id = %started.session_id,
                "scheduled round dispatched"
            );
        }
        Err(err) => tracing::warn!(%err, round_type, project_id, "scheduled round failed to dispatch"),
    }
}

/// Parse a round schedule json, returning the interval in minutes only when the schedule
/// is present and `enabled` is true. Everything else (absent, malformed, disabled, or a
/// non-positive interval) returns `None`.
fn parse_schedule_interval(schedule_json: Option<&str>) -> Option<u64> {
    let json = schedule_json?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if !value.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        return None;
    }
    let minutes = value.get("interval_minutes").and_then(|v| v.as_u64()).unwrap_or(0);
    // A zero interval means "every tick"; keep it, so tests and eager schedules work.
    Some(minutes)
}

/// Milliseconds since the Unix epoch (scheduler bookmarks).
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

async fn health() -> &'static str {
    "ok"
}

/// `POST /shutdown` with `Authorization: Bearer <root-token>`: trigger graceful
/// shutdown for `dflowd --stop`. Loopback only; a wrong/missing token is rejected.
async fn shutdown_handler(State(state): State<AppState>, headers: HeaderMap) -> StatusCode {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let expected = format!("Bearer {}", state.token.as_str());
    if auth == expected {
        tracing::info!("graceful shutdown requested via /shutdown");
        state.shutdown.notify_waiters();
        StatusCode::OK
    } else {
        StatusCode::UNAUTHORIZED
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    // Loopback origin: the full token surface (root/agent/concertmaster/round). The LAN
    // listener uses its own handler with `lan_origin = true` to restrict to phone tokens.
    ws.on_upgrade(move |socket| handle_socket(socket, state, false)).into_response()
}

/// `POST /pair` with `Authorization: Bearer <root-token>`: enable the LAN listener (if it
/// is not already up) and mint a phone pairing, returning the `LanPairing` JSON. Backs
/// `dflowd --pair` so the runbook has a usable pairing path without a WS client. Loopback
/// only; a wrong/missing token is rejected.
async fn pair_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    if auth != format!("Bearer {}", state.token.as_str()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    // Ensure the LAN listener is up so the pairing URL is reachable.
    if !state.lan.is_bound() {
        if let Err(err) = crate::api::lan_enable(state.clone(), dflow_proto::LanEnable { port: None }).await {
            return (StatusCode::INTERNAL_SERVER_ERROR, format!("could not enable LAN: {err}")).into_response();
        }
    }
    match crate::api::lan_pair(&state, dflow_proto::LanPair { name: None }) {
        Ok(pairing) => axum::Json(pairing).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("pairing failed: {err}")).into_response(),
    }
}
