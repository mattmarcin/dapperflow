//! Per-connection handling: the auth handshake, the control-message dispatch, and
//! binary PTY frame routing.
//!
//! The key persistence guarantee lives here: when a connection drops, every
//! attached session is *detached*, never killed. The sessions keep running in the
//! `SessionManager`, and a later connection replays their scrollback on reattach
//! (`architecture.md` / two-process model).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use base64::Engine;
use dflow_core::{default_command, harness, SessionSpec};
use dflow_proto::{
    decode_frame, encode_frame, AgentAdd, AgentRemove, AgentUpdate, AgentsDetect, AgentsList,
    ArtifactGet, ArtifactRegister, AuthHello, AuthWelcome, CardCreate, CardGet, CardMove, CardQuery,
    CardUpdate, ClientKind, DispatchCancel, DispatchStart, EnvCleanup, EnvDelete, EnvImport, EnvList,
    EnvMaterialize, EnvSet, Envelope, EventAck, EventCardEvent, EventSubscribe, EventSubscribed,
    FeedbackPoll, FeedbackSubmit, FindingAdd, FrameKind, GateMerge, GateResolveFinding, GateRun,
    GateShip, GateStatus, GithubAuthStatus, GithubIssueGet, GithubIssuesImport, GithubIssuesPreview,
    KnowAdd, KnowFind, KnowGet, KnowIndex, LanDisable, LanEnable, LanPair, LanRevoke, LanStatus,
    MintConcertmaster, NeedsYouList, NeedsYouResolve,
    NotifyForward, ProjectAdd, ProjectList, ProjectUpdate, ProtocolError, RecipeGet,
    RecipeGrant as RecipeGrantMsg, RecipeInstall, RecipeList, RecipeRevokeGrant, RecipeValidate,
    RoundDigest, RoundStart, SelfReport, SendVerified, ServiceAdd, ServiceList, ServiceRemove,
    SessionAttach, SessionAttached, SessionCreate, SessionCreated, SessionDetach, SessionKill,
    SessionPeek, SessionRename, SessionResume, SetNote, Simple, CLOSE_AUTH_FAILED,
    CLOSE_UPGRADE_REQUIRED, PROTOCOL_VERSION,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinHandle;
use ulid::Ulid;

use crate::api;
use crate::server::AppState;
use crate::tokens::{AgentToken, RoundToken};

/// The authenticated scope of a connection (`security.md` / Token architecture).
enum ConnScope {
    /// The desktop app or the MCP server (root token): the full protocol surface. `mcp`
    /// marks a Concertmaster-scoped client so the daemon *attributes* its steers
    /// (`concertmaster_steered`) even though it authenticates with the root token
    /// (`phase6-mcp.md` merge-time request 1).
    Root { mcp: bool },
    /// The `dflow` agent CLI (per-task token): a scoped allowlist of verbs bounded to
    /// its own card, session, and project.
    Agent(Arc<AgentToken>),
    /// A Concertmaster-scoped token (`security.md` / Concertmaster capability scope):
    /// the read + board + dispatch + steer + knowledge surface, with vault, kill, and
    /// merge-class verbs excluded and enforced by `dispatch_concertmaster`.
    Concertmaster,
    /// A round-scoped token (`product.md` / Concertmaster rounds; M4): the Concertmaster
    /// read surface plus exactly one write verb, `round.digest`, bound to the token's own
    /// round card. Enforced by `dispatch_round`.
    Round(Arc<RoundToken>),
    /// A phone-scoped capability token (`security.md` / Remote access trust model; M6):
    /// the attention surface only (Needs You, read-only fleet/peek, approvals, steering),
    /// with vault, kill, dispatch, agents, recipes, and daemon verbs excluded. Enforced
    /// by `dispatch_phone`. `String` is the pairing label, for logging. Only ever granted
    /// over the LAN listener.
    Phone(String),
}

const BASE64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Page size when replaying card events to a resuming subscriber.
const EVENT_REPLAY_PAGE: i64 = 500;

/// Drive a single client connection to completion. `lan_origin` is true for connections
/// arriving on the opt-in LAN listener, which restricts the handshake to phone-scoped
/// tokens (`security.md`: the root/task/round/concertmaster tokens never work over LAN).
pub async fn handle_socket(socket: WebSocket, state: AppState, lan_origin: bool) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();

    // One writer task owns the sink; every producer sends through `out_tx`.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // The first frame must be a valid auth.hello (`protocol.md` / Authentication).
    let scope = match authenticate(&mut stream, &state, lan_origin).await {
        Ok((scope, welcome)) => {
            let _ = out_tx.send(text(&welcome));
            scope
        }
        Err((code, reason)) => {
            tracing::warn!(code, %reason, "auth handshake rejected");
            let _ = out_tx.send(Message::Close(Some(CloseFrame { code, reason: reason.into() })));
            drop(out_tx);
            let _ = writer.await;
            return;
        }
    };

    // session ulid -> output forwarder task for that attachment.
    let mut attached: HashMap<Ulid, JoinHandle<()>> = HashMap::new();
    // This connection's card-event stream task, if it subscribed (`event.subscribe`).
    let mut event_sub: Option<JoinHandle<()>> = None;

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Text(text) => {
                if dispatch_control(
                    text.as_str(),
                    &state,
                    &scope,
                    &out_tx,
                    &mut attached,
                    &mut event_sub,
                )
                .await
                {
                    break; // shutdown requested
                }
            }
            // Only the desktop (root) drives PTYs; neither a task token nor a
            // Concertmaster token sends PTY frames, so they can never write into another
            // session's terminal by frame id (a Concertmaster steers only through the
            // guarded `session.send_verified` verb).
            Message::Binary(bytes) => {
                if matches!(scope, ConnScope::Root { .. }) {
                    route_binary(&bytes, &state);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Connection ending: detach everything (sessions keep running).
    for (id, handle) in attached.drain() {
        handle.abort();
        if let Some(session) = state.sessions.get(&id) {
            session.mark_detached();
        }
    }
    if let Some(handle) = event_sub.take() {
        handle.abort();
    }
    drop(out_tx);
    let _ = writer.await;
}

/// Await and validate the auth.hello handshake, resolving the connection's scope.
///
/// The root token grants the full desktop surface; a per-task token presented by an
/// `agent` client grants the scoped agent surface (`security.md` / Per-task tokens). An
/// unknown or revoked token closes with `CLOSE_AUTH_FAILED`, which the CLI maps to its
/// "token expired/revoked" exit code.
async fn authenticate(
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
    state: &AppState,
    lan_origin: bool,
) -> Result<(ConnScope, Envelope), (u16, String)> {
    let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.next()).await;
    let msg = match first {
        Ok(Some(Ok(m))) => m,
        _ => return Err((CLOSE_AUTH_FAILED, "handshake timed out or connection closed".into())),
    };
    let text = match msg {
        Message::Text(t) => t,
        _ => return Err((CLOSE_AUTH_FAILED, "first frame must be auth.hello (text)".into())),
    };
    let env: Envelope = serde_json::from_str(text.as_str())
        .map_err(|e| (CLOSE_AUTH_FAILED, format!("malformed handshake envelope: {e}")))?;
    if env.msg_type != "auth.hello" {
        return Err((CLOSE_AUTH_FAILED, "first message must be auth.hello".into()));
    }
    let hello: AuthHello = env
        .decode_payload()
        .map_err(|e| (CLOSE_AUTH_FAILED, format!("bad auth.hello payload: {e}")))?;
    if !hello.proto_versions.contains(&PROTOCOL_VERSION) {
        return Err((CLOSE_UPGRADE_REQUIRED, "no shared protocol version".into()));
    }
    // The LAN listener accepts ONLY phone-scoped capability tokens: the root, task,
    // round, and Concertmaster tokens are loopback-only and never usable over the LAN,
    // a hard boundary beyond the capability gate (`security.md` / Remote access).
    if lan_origin {
        let phone = state
            .store
            .resolve_phone_token(&hello.token)
            .ok()
            .flatten()
            .ok_or((CLOSE_AUTH_FAILED, "invalid or revoked phone token".to_string()))?;
        let label = phone.name.clone().unwrap_or_else(|| "phone".to_string());
        let welcome = AuthWelcome {
            proto_version: PROTOCOL_VERSION,
            scope: "phone".into(),
            daemon_version: state.daemon_version.as_str().to_string(),
        };
        return Ok((
            ConnScope::Phone(label),
            Envelope::message(env.id.unwrap_or_default(), "auth.welcome", welcome),
        ));
    }
    let (scope, label) = if tokens_match(&hello.token, state.token.as_str()) {
        // The root token grants the full surface; an `mcp` client marker only changes
        // attribution (concertmaster_steered), never scope.
        (ConnScope::Root { mcp: hello.client == ClientKind::Mcp }, "root")
    } else if state.concertmaster.contains(&hello.token) {
        (ConnScope::Concertmaster, "concertmaster")
    } else if let Some(round) = state.round.resolve(&hello.token) {
        (ConnScope::Round(round), "round")
    } else if hello.client == ClientKind::Agent {
        match state.tokens.resolve(&hello.token) {
            Some(tok) => (ConnScope::Agent(tok), "agent"),
            None => return Err((CLOSE_AUTH_FAILED, "invalid or revoked task token".into())),
        }
    } else {
        return Err((CLOSE_AUTH_FAILED, "invalid token".into()));
    };
    let welcome = AuthWelcome {
        proto_version: PROTOCOL_VERSION,
        scope: label.into(),
        daemon_version: state.daemon_version.as_str().to_string(),
    };
    Ok((scope, Envelope::message(env.id.unwrap_or_default(), "auth.welcome", welcome)))
}

/// Length-aware constant-time-ish token comparison. Phase 0 keeps this simple;
/// a hardened comparison is a later-phase hardening item (`security.md`).
fn tokens_match(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Handle one control message. Returns `true` if the daemon should shut down.
async fn dispatch_control(
    raw: &str,
    state: &AppState,
    scope: &ConnScope,
    out_tx: &UnboundedSender<Message>,
    attached: &mut HashMap<Ulid, JoinHandle<()>>,
    event_sub: &mut Option<JoinHandle<()>>,
) -> bool {
    let env: Envelope = match serde_json::from_str(raw) {
        Ok(e) => e,
        Err(e) => {
            send(out_tx, &Envelope::error(None, ProtocolError::bad_request(format!("bad envelope: {e}"))));
            return false;
        }
    };
    let id = env.id.clone().unwrap_or_default();

    // Route by scope. A per-task (agent) token gets only the scoped agent surface; a
    // Concertmaster token gets the reduced Concertmaster surface; every other verb is a
    // Forbidden. A root connection falls through to the full table; `mcp` only marks
    // whether its steers are attributed to the Concertmaster.
    let mcp = match scope {
        ConnScope::Agent(token) => return dispatch_agent(&env, &id, token, state, out_tx).await,
        ConnScope::Concertmaster => return dispatch_concertmaster(&env, &id, state, out_tx).await,
        ConnScope::Round(token) => return dispatch_round(&env, &id, token, state, out_tx).await,
        ConnScope::Phone(label) => {
            tracing::trace!(phone = %label, verb = %env.msg_type, "phone-scoped verb");
            return dispatch_phone(&env, &id, state, out_tx, event_sub).await;
        }
        ConnScope::Root { mcp } => *mcp,
    };

    match env.msg_type.as_str() {
        "session.create" => {
            let payload: SessionCreate = match env.decode_payload() {
                Ok(p) => p,
                Err(e) => return reply_err(out_tx, &id, ProtocolError::bad_request(e.to_string())),
            };
            // Resolution: an explicit command wins; else a configured `agent` launcher
            // (interactive, no brief) contributes its argv, harness, extra env, and
            // launcher id; else the Phase 0 default shell for the harness name.
            let explicit = payload.command.clone().filter(|c| !c.is_empty());
            let mut harness = payload.harness.clone();
            let mut env = payload.env.clone();
            let mut agent_id: Option<String> = None;
            let command = if let Some(cmd) = explicit {
                cmd
            } else if let Some(agent_ref) = payload.agent.as_deref().filter(|s| !s.trim().is_empty()) {
                match api::resolve_agent_launch(state, agent_ref) {
                    Ok(launch) => {
                        harness = launch.harness;
                        // Launcher env wins over the client-supplied base env.
                        env.extend(launch.env);
                        agent_id = launch.agent_id;
                        launch.command
                    }
                    Err(e) => return reply_err(out_tx, &id, e),
                }
            } else {
                match default_command(&payload.harness) {
                    Some(c) if !c.is_empty() => c,
                    _ => {
                        return reply_err(
                            out_tx,
                            &id,
                            ProtocolError::bad_request(format!("unknown harness '{}' and no command given", payload.harness)),
                        )
                    }
                }
            };
            // Cardless (bare) sessions persist now, so they survive a daemon restart
            // with their Projects-tree identity: link the session to a card if the
            // client named one, else match cwd -> project (Phase 2 reconciliation).
            let cwd = payload.cwd.clone().map(PathBuf::from);
            let card_id = payload
                .card_id
                .as_deref()
                .and_then(|c| Ulid::from_string(c).ok());
            let project_id = if card_id.is_some() {
                None
            } else {
                cwd.as_deref().and_then(|c| api::find_project_for_path(state, c))
            };
            let first_prompt = payload
                .first_prompt
                .as_deref()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(|p| p.to_string());
            // Mint a per-task token so a New Session agent can self-report and maintain
            // the board through `dflow` too, and inject the agent-CLI env before spawn
            // (env can only enter a process at spawn).
            let card_ref = card_id.map(|c| c.to_string());
            let (task_token, token_handle) = state.tokens.mint(crate::tokens::TokenScope {
                card_id: card_ref.clone(),
                project_id: project_id.clone(),
                audit: false,
                budget_cards: None,
                budget_notes: None,
                recipe: None,
                gate_run_id: None,
            });
            api::inject_agent_env(state, &mut env, &task_token, card_ref.as_deref());
            let spec = SessionSpec {
                harness,
                command,
                cols: payload.cols.max(1),
                rows: payload.rows.max(1),
                cwd,
                env,
                card_id,
                project_id,
                agent_id,
                first_prompt: first_prompt.as_deref().map(|p| harness::preview(p, 120)),
                scrollback_dir: Some(state.data_dir.scrollback_dir()),
                ..Default::default()
            };
            match state.sessions.create(spec) {
                Ok(session) => {
                    tracing::info!(session_id = %session.id, "session created");
                    // Bind the per-task token to the freshly spawned session.
                    token_handle.bind_session(&session.id.to_string());
                    // Wire the New Session first prompt through verified submit once the
                    // composer is ready (`adapters.md` / Verified submit). The submit
                    // runs in the background; failure raises Needs You, never silently
                    // drops the message.
                    let queued = first_prompt.is_some();
                    if let Some(prompt) = first_prompt {
                        api::spawn_first_prompt_submit(state, Arc::clone(&session), prompt);
                    }
                    send(out_tx, &Envelope::message(id, "session.create", SessionCreated { session_id: session.id.to_string(), first_prompt_queued: queued }));
                }
                Err(e) => {
                    reply_err(out_tx, &id, ProtocolError::internal(e.to_string()));
                }
            }
        }
        "session.attach" => {
            let payload: SessionAttach = match env.decode_payload() {
                Ok(p) => p,
                Err(e) => return reply_err(out_tx, &id, ProtocolError::bad_request(e.to_string())),
            };
            let session = match state.sessions.get_str(&payload.session_id) {
                Some(s) => s,
                None => return reply_err(out_tx, &id, ProtocolError::not_found("no such session")),
            };

            // Match the PTY to the client's terminal size, then take a lossless
            // handoff of replay bytes + a live subscription.
            let _ = session.resize(payload.cols.max(1), payload.rows.max(1));
            let (replay, mut rx) = session.attach();
            let (cols, rows) = session.size();
            let resp = SessionAttached {
                session_id: payload.session_id.clone(),
                cols,
                rows,
                cursor: session.cursor(),
                snapshot: session.styled_snapshot(),
                replay_base64: BASE64.encode(&replay),
            };
            send(out_tx, &Envelope::message(id, "session.attach", resp));

            // Forward live output as binary frames until the socket or session ends.
            let sid = session.id_bytes();
            let out = out_tx.clone();
            let session_for_lag = Arc::clone(&session);
            let handle = tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(chunk) => {
                            let frame = encode_frame(FrameKind::Output, &sid, &chunk);
                            if out.send(Message::Binary(frame.into())).is_err() {
                                break;
                            }
                        }
                        Err(RecvError::Lagged(dropped)) => {
                            // A slow client: hand it a fresh mode-aware repaint instead
                            // of an unbounded buffer (`protocol.md` / flow control).
                            tracing::debug!(dropped, "client lagged; sending fresh replay");
                            let fresh = session_for_lag.repaint_payload();
                            let frame = encode_frame(FrameKind::Output, &sid, &fresh);
                            if out.send(Message::Binary(frame.into())).is_err() {
                                break;
                            }
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
            });
            if let Some(old) = attached.insert(session.id, handle) {
                old.abort();
                session.mark_detached();
            }
            // Authoritative repaint nudge (Phase 2 reattach fix): a resize jiggle
            // (width-1 then back) forces the child TUI to repaint its full screen from
            // its own state, correcting anything the snapshot replay could not capture.
            // ConPTY-safe; the child ends at the client's real width.
            let (cols, rows) = session.size();
            if cols > 1 {
                let _ = session.resize(cols - 1, rows);
                let _ = session.resize(cols, rows);
            }
            tracing::info!(session_id = %session.id, "client attached");
        }
        "session.detach" => {
            let payload: SessionDetach = match env.decode_payload() {
                Ok(p) => p,
                Err(e) => return reply_err(out_tx, &id, ProtocolError::bad_request(e.to_string())),
            };
            if let Ok(ulid) = Ulid::from_string(&payload.session_id) {
                if let Some(handle) = attached.remove(&ulid) {
                    handle.abort();
                    if let Some(session) = state.sessions.get(&ulid) {
                        session.mark_detached();
                    }
                }
            }
            send(out_tx, &Envelope::message(id, "session.detach", Simple::ok()));
        }
        "session.kill" => {
            let payload: SessionKill = match env.decode_payload() {
                Ok(p) => p,
                Err(e) => return reply_err(out_tx, &id, ProtocolError::bad_request(e.to_string())),
            };
            match Ulid::from_string(&payload.session_id) {
                Ok(ulid) => {
                    if let Some(handle) = attached.remove(&ulid) {
                        handle.abort();
                    }
                    let killed = state.sessions.kill(&ulid);
                    tracing::info!(session_id = %payload.session_id, killed, "session kill requested");
                    send(out_tx, &Envelope::message(id, "session.kill", Simple { ok: killed }));
                }
                Err(_) => {
                    reply_err(out_tx, &id, ProtocolError::bad_request("invalid session id"));
                }
            }
        }
        "session.list" => match api::session_list(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id, "session.list", resp)),
            Err(e) => {
                reply_err(out_tx, &id, e);
            }
        },
        "fleet.status" => match api::fleet_status(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id, "fleet.status", resp)),
            Err(e) => {
                reply_err(out_tx, &id, e);
            }
        },
        "session.rename" => {
            handle_request::<SessionRename, _, _>(&env, out_tx, &id, |p| {
                api::session_rename(state, p)
            });
        }
        "session.resume" => {
            handle_request::<SessionResume, _, _>(&env, out_tx, &id, |p| {
                api::session_resume(state, p)
            });
        }
        "agents.list" => {
            handle_request::<AgentsList, _, _>(&env, out_tx, &id, |_p| api::agents_list(state));
        }
        "agents.add" => {
            handle_request::<AgentAdd, _, _>(&env, out_tx, &id, |p| api::agents_add(state, p));
        }
        "agents.update" => {
            handle_request::<AgentUpdate, _, _>(&env, out_tx, &id, |p| api::agents_update(state, p));
        }
        "agents.remove" => {
            handle_request::<AgentRemove, _, _>(&env, out_tx, &id, |p| api::agents_remove(state, p));
        }
        "agents.detect" => {
            handle_request::<AgentsDetect, _, _>(&env, out_tx, &id, |_p| api::agents_detect(state));
        }
        "project.add" => {
            handle_request::<ProjectAdd, _, _>(&env, out_tx, &id, |p| api::project_add(state, p));
        }
        "project.update" => {
            handle_request::<ProjectUpdate, _, _>(&env, out_tx, &id, |p| {
                api::project_update(state, p)
            });
        }
        "project.list" => {
            handle_request::<ProjectList, _, _>(&env, out_tx, &id, |p| api::project_list(state, p));
        }
        "card.create" => {
            handle_request::<CardCreate, _, _>(&env, out_tx, &id, |p| api::card_create(state, p));
        }
        "card.update" => {
            handle_request::<CardUpdate, _, _>(&env, out_tx, &id, |p| api::card_update(state, p));
        }
        "card.move" => {
            handle_request::<CardMove, _, _>(&env, out_tx, &id, |p| api::card_move(state, p));
        }
        "card.query" => {
            handle_request::<CardQuery, _, _>(&env, out_tx, &id, |p| api::card_query(state, p));
        }
        "card.get" => {
            handle_request::<CardGet, _, _>(&env, out_tx, &id, |p| api::card_get(state, p));
        }
        "dispatch.start" => {
            handle_request::<DispatchStart, _, _>(&env, out_tx, &id, |p| {
                api::dispatch_start(state, p)
            });
        }
        "dispatch.cancel" => {
            handle_request::<DispatchCancel, _, _>(&env, out_tx, &id, |p| {
                api::dispatch_cancel(state, p)
            });
        }
        // env.* (the env vault, `environments.md`). Owner scope only: a per-task token
        // and a Concertmaster token both lack these entirely.
        "env.set" => {
            handle_request::<EnvSet, _, _>(&env, out_tx, &id, |p| api::env_set(state, p));
        }
        "env.list" => {
            handle_request::<EnvList, _, _>(&env, out_tx, &id, |p| api::env_list(state, p));
        }
        "env.delete" => {
            handle_request::<EnvDelete, _, _>(&env, out_tx, &id, |p| api::env_delete(state, p));
        }
        "env.materialize" => {
            handle_request::<EnvMaterialize, _, _>(&env, out_tx, &id, |p| api::env_materialize(state, p));
        }
        "env.cleanup" => {
            handle_request::<EnvCleanup, _, _>(&env, out_tx, &id, |p| api::env_cleanup(state, p));
        }
        "env.import" => {
            handle_request::<EnvImport, _, _>(&env, out_tx, &id, |p| api::env_import(state, p));
        }
        // session.peek: read-only scrubbed screen capture, no PTY resize (`phase6-mcp.md`).
        "session.peek" => {
            handle_request::<SessionPeek, _, _>(&env, out_tx, &id, |p| api::session_peek(state, p));
        }
        // session.send_verified: guarded steering. Emits concertmaster_steered when the
        // caller is mcp-scoped (`phase6-mcp.md` merge-time request 1). Runs the blocking
        // verified submit off the async executor.
        "session.send_verified" => {
            handle_send_verified(&env, state, &id, out_tx, mcp).await;
        }
        // know.* on owner scope (`phase6-mcp.md` merge-time request 2): the explicit
        // project_id in each request addresses the project (agent tokens use their own).
        "know.index" => {
            handle_request::<KnowIndex, _, _>(&env, out_tx, &id, |p| api::know_index(state, None, p));
        }
        "know.find" => {
            handle_request::<KnowFind, _, _>(&env, out_tx, &id, |p| api::know_find(state, None, p));
        }
        "know.get" => {
            handle_request::<KnowGet, _, _>(&env, out_tx, &id, |p| api::know_get(state, None, p));
        }
        // Mint a Concertmaster-scoped token (`security.md` / Concertmaster capability
        // scope; `phase6-mcp.md` merge-time request 4).
        "auth.mint_concertmaster" => {
            handle_request::<MintConcertmaster, _, _>(&env, out_tx, &id, |_p| api::mint_concertmaster(state));
        }
        // round.start: dispatch a headless Concertmaster round (`product.md` / rounds).
        "round.start" => {
            handle_request::<RoundStart, _, _>(&env, out_tx, &id, |p| api::round_start(state, p));
        }
        // needs_you.list / resolve: the attention queue as a first-class pair (also on
        // the phone scope). Resolving stamps `resolved_by: ui` for a desktop caller.
        "needs_you.list" => {
            handle_request::<NeedsYouList, _, _>(&env, out_tx, &id, |_p| api::needs_you_list(state));
        }
        "needs_you.resolve" => {
            handle_request::<NeedsYouResolve, _, _>(&env, out_tx, &id, |p| {
                api::needs_you_resolve(state, p, "ui")
            });
        }
        // daemon.lan.*: the opt-in LAN listener (`security.md` / Remote access; M6).
        // enable/disable bind or stop the second listener; pair mints a phone token and
        // returns the QR payload; revoke drops a pairing. Loopback (owner) scope only.
        "daemon.lan.enable" => {
            handle_async_request::<LanEnable, _, _, _>(&env, out_tx, &id, |p| {
                api::lan_enable(state.clone(), p)
            })
            .await;
        }
        "daemon.lan.disable" => {
            handle_request::<LanDisable, _, _>(&env, out_tx, &id, |_p| api::lan_disable(state));
        }
        "daemon.lan.status" => {
            handle_request::<LanStatus, _, _>(&env, out_tx, &id, |_p| api::lan_status(state));
        }
        "daemon.lan.pair" => {
            handle_request::<LanPair, _, _>(&env, out_tx, &id, |p| api::lan_pair(state, p));
        }
        "daemon.lan.revoke" => {
            handle_request::<LanRevoke, _, _>(&env, out_tx, &id, |p| api::lan_revoke(state, p));
        }
        "recipe.list" => {
            handle_request::<RecipeList, _, _>(&env, out_tx, &id, |p| {
                crate::recipes::recipe_list(state, p)
            });
        }
        "recipe.get" => {
            handle_request::<RecipeGet, _, _>(&env, out_tx, &id, |p| {
                crate::recipes::recipe_get(state, p)
            });
        }
        "recipe.validate" => {
            handle_request::<RecipeValidate, _, _>(&env, out_tx, &id, |p| {
                Ok::<_, ProtocolError>(crate::recipes::recipe_validate(p))
            });
        }
        "recipe.install" => {
            handle_request::<RecipeInstall, _, _>(&env, out_tx, &id, |p| {
                crate::recipes::recipe_install(state, p)
            });
        }
        "recipe.grant" => {
            handle_request::<RecipeGrantMsg, _, _>(&env, out_tx, &id, |p| {
                crate::recipes::recipe_grant(state, p)
            });
        }
        "recipe.revoke_grant" => {
            handle_request::<RecipeRevokeGrant, _, _>(&env, out_tx, &id, |p| {
                crate::recipes::recipe_revoke_grant(state, p)
            });
        }
        // artifact.* (Plan Studio, `plan-studio.md`). The desktop registers/gets/submits;
        // the poll is a bounded long-poll (root passes an explicit artifact_id).
        "artifact.register" => {
            handle_request::<ArtifactRegister, _, _>(&env, out_tx, &id, |p| {
                api::artifact_register(state, p)
            });
        }
        "artifact.get" => {
            handle_request::<ArtifactGet, _, _>(&env, out_tx, &id, |p| api::artifact_get(state, p));
        }
        "artifact.feedback.submit" => {
            handle_request::<FeedbackSubmit, _, _>(&env, out_tx, &id, |p| {
                api::feedback_submit(state, p)
            });
        }
        "artifact.feedback.poll" => {
            handle_artifact_poll(&env, state, &id, out_tx, None).await;
        }
        // github.* (`roadmap.md` M5.1-2): gh presence/auth reporting and read-only issue
        // import. Owner scope only; the gh CLI is the transport (dflow-core::github).
        "github.auth.status" => {
            handle_request::<GithubAuthStatus, _, _>(&env, out_tx, &id, |p| {
                crate::github::github_auth_status(state, p)
            });
        }
        "github.issues.preview" => {
            handle_request::<GithubIssuesPreview, _, _>(&env, out_tx, &id, |p| {
                crate::github::github_issues_preview(state, p)
            });
        }
        "github.issues.import" => {
            handle_request::<GithubIssuesImport, _, _>(&env, out_tx, &id, |p| {
                crate::github::github_issues_import(state, p)
            });
        }
        "github.issue.get" => {
            handle_request::<GithubIssueGet, _, _>(&env, out_tx, &id, |p| {
                crate::github::github_issue_get(state, p)
            });
        }
        // gate.* (`gate.md` / Pipeline): the verification gate. Owner scope drives it;
        // the pipeline runs on a background thread and streams progress as card events.
        "gate.run" => {
            handle_request::<GateRun, _, _>(&env, out_tx, &id, |p| crate::gate::gate_run(state, p));
        }
        "gate.status" => {
            handle_request::<GateStatus, _, _>(&env, out_tx, &id, |p| crate::gate::gate_status(state, p));
        }
        "gate.resolve_finding" => {
            handle_request::<GateResolveFinding, _, _>(&env, out_tx, &id, |p| {
                crate::gate::gate_resolve_finding(state, p)
            });
        }
        // Ship path (`gate.md` / Ship): push + PR (pr mode) or local fast-forward merge,
        // then CI-watch + merge + landed-work teardown. Owner-only (merge/push are never
        // Concertmaster actions, `security.md`).
        "gate.ship" => {
            handle_request::<GateShip, _, _>(&env, out_tx, &id, |p| crate::gate::gate_ship(state, p));
        }
        "gate.merge" => {
            handle_request::<GateMerge, _, _>(&env, out_tx, &id, |p| crate::gate::gate_merge(state, p));
        }
        // service.* (per-project local services, `environments.md`). Owner scope only.
        "service.add" => {
            handle_request::<ServiceAdd, _, _>(&env, out_tx, &id, |p| api::service_add(state, p));
        }
        "service.list" => {
            handle_request::<ServiceList, _, _>(&env, out_tx, &id, |p| api::service_list(state, p));
        }
        "service.remove" => {
            handle_request::<ServiceRemove, _, _>(&env, out_tx, &id, |p| api::service_remove(state, p));
        }
        "event.subscribe" => {
            let payload: EventSubscribe = match env.decode_payload() {
                Ok(p) => p,
                Err(e) => return reply_err(out_tx, &id, ProtocolError::bad_request(e.to_string())),
            };
            start_event_stream(state, out_tx, &id, payload, event_sub);
        }
        "event.ack" => {
            // v0: the client persists its own cursor; the ack is acknowledged so the
            // verb exists on the wire (`protocol.md`). Server-side bookmarks arrive
            // with multi-client sync.
            let ok = env.decode_payload::<EventAck>().is_ok();
            if ok {
                send(out_tx, &Envelope::message(id, "event.ack", Simple::ok()));
            } else {
                reply_err(out_tx, &id, ProtocolError::bad_request("event.ack needs { cursor }"));
            }
        }
        "daemon.shutdown" => {
            send(out_tx, &Envelope::message(id, "daemon.shutdown", Simple::ok()));
            state.shutdown.notify_waiters();
            return true;
        }
        "auth.hello" => {
            reply_err(out_tx, &id, ProtocolError::bad_request("already authenticated"));
        }
        other => {
            reply_err(
                out_tx,
                &id,
                ProtocolError::unsupported(format!("verb '{other}' is not implemented yet")),
            );
        }
    }
    false
}

/// Dispatch a control message from a per-task (agent) token: only the scoped agent
/// surface (`agent-cli.md` verbs, `knowledge.md` know verbs), each bounded to the
/// token's own card/session/project. Every other verb returns a Forbidden. Never
/// shuts the daemon down, so it always returns `false`.
async fn dispatch_agent(
    env: &Envelope,
    id: &str,
    token: &Arc<AgentToken>,
    state: &AppState,
    out_tx: &UnboundedSender<Message>,
) -> bool {
    match env.msg_type.as_str() {
        "agent.context" => match api::agent_context(state, token) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "agent.context", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "session.self_report" => {
            handle_request::<SelfReport, _, _>(env, out_tx, id, |p| {
                api::session_self_report(state, token, p)
            });
        }
        "session.set_note" => {
            handle_request::<SetNote, _, _>(env, out_tx, id, |p| api::session_set_note(state, token, p));
        }
        "card.create" => {
            handle_request::<CardCreate, _, _>(env, out_tx, id, |p| {
                api::card_create_scoped(state, token, p)
            });
        }
        "card.update" => {
            handle_request::<CardUpdate, _, _>(env, out_tx, id, |p| {
                api::card_update_scoped(state, token, p)
            });
        }
        "card.move" => {
            handle_request::<CardMove, _, _>(env, out_tx, id, |p| api::card_move_scoped(state, token, p));
        }
        "card.get" => {
            handle_request::<CardGet, _, _>(env, out_tx, id, |p| api::card_get_scoped(state, token, p));
        }
        "know.index" => {
            handle_request::<KnowIndex, _, _>(env, out_tx, id, |p| api::know_index(state, Some(token), p));
        }
        "know.find" => {
            handle_request::<KnowFind, _, _>(env, out_tx, id, |p| api::know_find(state, Some(token), p));
        }
        "know.get" => {
            handle_request::<KnowGet, _, _>(env, out_tx, id, |p| api::know_get(state, Some(token), p));
        }
        "know.add" => {
            handle_request::<KnowAdd, _, _>(env, out_tx, id, |p| api::know_add(state, Some(token), p));
        }
        "notify.forward" => {
            handle_request::<NotifyForward, _, _>(env, out_tx, id, |p| {
                api::notify_forward(state, token, p)
            });
        }
        // finding.add: a gate reviewer session files a structured finding against its run
        // (`gate.md` / Adversarial review). The token carries the gate run id.
        "finding.add" => {
            handle_request::<FindingAdd, _, _>(env, out_tx, id, |p| {
                crate::gate::finding_add(state, token, p)
            });
        }
        // artifact.* (Plan Studio, agent side): `dflow plan open` registers, `dflow plan
        // poll` long-polls for the queued feedback batch. The token resolves the card.
        "artifact.register" => {
            handle_request::<ArtifactRegister, _, _>(env, out_tx, id, |p| {
                api::artifact_register_scoped(state, token, p)
            });
        }
        "artifact.feedback.poll" => {
            handle_artifact_poll(env, state, id, out_tx, Some(token)).await;
        }
        "auth.hello" => {
            reply_err(out_tx, id, ProtocolError::bad_request("already authenticated"));
        }
        other => {
            reply_err(
                out_tx,
                id,
                ProtocolError::forbidden(format!("verb '{other}' is not available to a task token")),
            );
        }
    }
    false
}

/// Handle `artifact.feedback.poll`: decode, run the bounded async long-poll, and reply.
/// `token` is `Some` for an agent connection (card-scoped, sets `awaiting_feedback`),
/// `None` for a root client (must pass an explicit `artifact_id`).
async fn handle_artifact_poll(
    env: &Envelope,
    state: &AppState,
    id: &str,
    out_tx: &UnboundedSender<Message>,
    token: Option<&Arc<AgentToken>>,
) {
    let payload: FeedbackPoll = match env.decode_payload() {
        Ok(p) => p,
        Err(e) => {
            reply_err(out_tx, id, ProtocolError::bad_request(e.to_string()));
            return;
        }
    };
    let token_ref = token.map(|t| t.as_ref());
    match api::artifact_feedback_poll(state, token_ref, payload).await {
        Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "artifact.feedback.poll", resp)),
        Err(e) => {
            reply_err(out_tx, id, e);
        }
    }
}

/// Dispatch a control message from a Concertmaster-scoped token (`security.md` /
/// Concertmaster capability scope; `phase6-mcp.md` merge-time request 4).
///
/// The allowlist is the Concertmaster's tool surface: fleet/session read, board query
/// and edits, dispatch, guarded steering, and knowledge. Everything else - the vault
/// (`env.*`), `session.kill`, `dispatch.cancel`, `agents.*`, `recipe.*`, and
/// `daemon.shutdown` - is a Forbidden, so the exclusions are daemon-enforced defense in
/// depth beyond the MCP server's own surface omission. Never shuts the daemon down.
async fn dispatch_concertmaster(
    env: &Envelope,
    id: &str,
    state: &AppState,
    out_tx: &UnboundedSender<Message>,
) -> bool {
    match env.msg_type.as_str() {
        "fleet.status" => match api::fleet_status(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "fleet.status", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "session.list" => match api::session_list(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "session.list", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "session.peek" => {
            handle_request::<SessionPeek, _, _>(env, out_tx, id, |p| api::session_peek(state, p));
        }
        "session.send_verified" => {
            // A Concertmaster steer is always attributed.
            handle_send_verified(env, state, id, out_tx, true).await;
        }
        "project.list" => {
            handle_request::<ProjectList, _, _>(env, out_tx, id, |p| api::project_list(state, p));
        }
        "card.query" => {
            handle_request::<CardQuery, _, _>(env, out_tx, id, |p| api::card_query(state, p));
        }
        "card.get" => {
            handle_request::<CardGet, _, _>(env, out_tx, id, |p| api::card_get(state, p));
        }
        "card.create" => {
            handle_request::<CardCreate, _, _>(env, out_tx, id, |p| api::card_create(state, p));
        }
        "card.update" => {
            handle_request::<CardUpdate, _, _>(env, out_tx, id, |p| api::card_update(state, p));
        }
        "card.move" => {
            handle_request::<CardMove, _, _>(env, out_tx, id, |p| api::card_move(state, p));
        }
        "dispatch.start" => {
            handle_request::<DispatchStart, _, _>(env, out_tx, id, |p| api::dispatch_start(state, p));
        }
        // The Concertmaster subsumes scheduling judgment: it can decide a round (or a
        // garden) is due from fleet activity (`product.md` / Concertmaster; `knowledge.md`).
        "round.start" => {
            handle_request::<RoundStart, _, _>(env, out_tx, id, |p| api::round_start(state, p));
        }
        "know.index" => {
            handle_request::<KnowIndex, _, _>(env, out_tx, id, |p| api::know_index(state, None, p));
        }
        "know.find" => {
            handle_request::<KnowFind, _, _>(env, out_tx, id, |p| api::know_find(state, None, p));
        }
        "know.get" => {
            handle_request::<KnowGet, _, _>(env, out_tx, id, |p| api::know_get(state, None, p));
        }
        "auth.hello" => {
            reply_err(out_tx, id, ProtocolError::bad_request("already authenticated"));
        }
        other => {
            reply_err(
                out_tx,
                id,
                ProtocolError::forbidden(format!(
                    "verb '{other}' is excluded from the Concertmaster scope (security.md: vault, \
                     kill, dispatch.cancel, agents.*, recipe.*, merge/push/discard, and \
                     daemon.shutdown are owner-only)"
                )),
            );
        }
    }
    false
}

/// Dispatch a control message from a round-scoped token (`product.md` / Concertmaster
/// rounds; M4).
///
/// The allowlist is the Concertmaster *read* surface (fleet/board/knowledge) so the
/// round agent can synthesize state, plus exactly one write verb - `round.digest` -
/// which files the round one escalation digest against the token's own round card.
/// Everything else is Forbidden, so a round session that goes off-script cannot dispatch,
/// steer, touch the vault, or move any card. Never shuts the daemon down.
async fn dispatch_round(
    env: &Envelope,
    id: &str,
    token: &Arc<RoundToken>,
    state: &AppState,
    out_tx: &UnboundedSender<Message>,
) -> bool {
    match env.msg_type.as_str() {
        // The one write verb: file this round's single escalation digest.
        "round.digest" => {
            handle_request::<RoundDigest, _, _>(env, out_tx, id, |p| {
                api::round_digest(state, token, p)
            });
        }
        // Read surface for synthesis (the round brief tells the agent to read fleet,
        // board, and knowledge before deciding what, if anything, to escalate).
        "fleet.status" => match api::fleet_status(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "fleet.status", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "session.list" => match api::session_list(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "session.list", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "session.peek" => {
            handle_request::<SessionPeek, _, _>(env, out_tx, id, |p| api::session_peek(state, p));
        }
        "project.list" => {
            handle_request::<ProjectList, _, _>(env, out_tx, id, |p| api::project_list(state, p));
        }
        "card.query" => {
            handle_request::<CardQuery, _, _>(env, out_tx, id, |p| api::card_query(state, p));
        }
        "card.get" => {
            handle_request::<CardGet, _, _>(env, out_tx, id, |p| api::card_get(state, p));
        }
        "know.index" => {
            handle_request::<KnowIndex, _, _>(env, out_tx, id, |p| api::know_index(state, None, p));
        }
        "know.find" => {
            handle_request::<KnowFind, _, _>(env, out_tx, id, |p| api::know_find(state, None, p));
        }
        "know.get" => {
            handle_request::<KnowGet, _, _>(env, out_tx, id, |p| api::know_get(state, None, p));
        }
        "auth.hello" => {
            reply_err(out_tx, id, ProtocolError::bad_request("already authenticated"));
        }
        other => {
            reply_err(
                out_tx,
                id,
                ProtocolError::forbidden(format!(
                    "verb '{other}' is not available to a round token (rounds may read fleet/board/\
                     knowledge and file one `round.digest`, nothing else)"
                )),
            );
        }
    }
    false
}

/// Dispatch a control message from a phone-scoped capability token (`security.md` /
/// Remote access trust model; M6; `mobile.md` capability profile).
///
/// The allowlist is the attention surface: the Needs You queue (list + resolve), the
/// read-only fleet (`fleet.status`, `session.list`), a scrubbed read-only terminal peek,
/// board reads for deep links, approvals + feedback on plan artifacts, one-shot steering,
/// and the event stream for live updates. Everything else - vault (`env.*`),
/// `session.kill`, `dispatch.*`, `agents.*`, `recipe.*`, and every `daemon.*` verb - is
/// Forbidden, exactly as `security.md` and `mobile.md` require, enforced daemon-side so a
/// stolen phone token cannot exceed the profile. Never shuts the daemon down.
async fn dispatch_phone(
    env: &Envelope,
    id: &str,
    state: &AppState,
    out_tx: &UnboundedSender<Message>,
    event_sub: &mut Option<JoinHandle<()>>,
) -> bool {
    match env.msg_type.as_str() {
        // Needs You: the phone home screen.
        "needs_you.list" => {
            handle_request::<NeedsYouList, _, _>(env, out_tx, id, |_p| api::needs_you_list(state));
        }
        "needs_you.resolve" => {
            handle_request::<NeedsYouResolve, _, _>(env, out_tx, id, |p| {
                api::needs_you_resolve(state, p, "mobile")
            });
        }
        // Read-only fleet + board.
        "fleet.status" => match api::fleet_status(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "fleet.status", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "session.list" => match api::session_list(state) {
            Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), "session.list", resp)),
            Err(e) => {
                reply_err(out_tx, id, e);
            }
        },
        "card.query" => {
            handle_request::<CardQuery, _, _>(env, out_tx, id, |p| api::card_query(state, p));
        }
        "card.get" => {
            handle_request::<CardGet, _, _>(env, out_tx, id, |p| api::card_get(state, p));
        }
        // Read-only terminal peek: a scrubbed screen capture, no PTY input path. The
        // scrubber runs on peek so no vault secret ever leaves over the LAN
        // (`security.md`: all phone traffic passes the secret scrubber on peek).
        "session.peek" => {
            handle_request::<SessionPeek, _, _>(env, out_tx, id, |p| api::session_peek(state, p));
        }
        // Approvals + plan review: render an artifact and submit the feedback/approve
        // batch. Merge stays gated in the client (M5); the daemon verb surface is here.
        "artifact.get" => {
            handle_request::<ArtifactGet, _, _>(env, out_tx, id, |p| api::artifact_get(state, p));
        }
        "artifact.feedback.submit" => {
            handle_request::<FeedbackSubmit, _, _>(env, out_tx, id, |p| api::feedback_submit(state, p));
        }
        // Steering: one-shot verified submit into a session (the phone profile in
        // `security.md` grants steering; attribution stays desktop-side, not phone).
        "session.send_verified" => {
            handle_send_verified(env, state, id, out_tx, false).await;
        }
        // Live updates: the event stream for cursor-replay Needs You updates (`mobile.md`
        // 3.3). Read-only; the phone persists its own cursor.
        "event.subscribe" => {
            let payload: EventSubscribe = match env.decode_payload() {
                Ok(p) => p,
                Err(e) => return reply_err(out_tx, id, ProtocolError::bad_request(e.to_string())),
            };
            start_event_stream(state, out_tx, id, payload, event_sub);
        }
        "event.ack" => {
            let ok = env.decode_payload::<EventAck>().is_ok();
            if ok {
                send(out_tx, &Envelope::message(id.to_string(), "event.ack", Simple::ok()));
            } else {
                reply_err(out_tx, id, ProtocolError::bad_request("event.ack needs { cursor }"));
            }
        }
        "auth.hello" => {
            reply_err(out_tx, id, ProtocolError::bad_request("already authenticated"));
        }
        other => {
            reply_err(
                out_tx,
                id,
                ProtocolError::forbidden(format!(
                    "verb '{other}' is excluded from the phone capability profile (security.md: \
                     no vault, kill, dispatch, agents, recipes, or daemon verbs; terminals are \
                     read-only)"
                )),
            );
        }
    }
    false
}

/// Handle `session.send_verified`: resolve the live session, run the blocking verified
/// submit off the async executor, and (for an mcp-scoped caller whose text reached the
/// composer) record the `concertmaster_steered` event (`phase6-mcp.md` request 1).
async fn handle_send_verified(
    env: &Envelope,
    state: &AppState,
    id: &str,
    out_tx: &UnboundedSender<Message>,
    attribute_concertmaster: bool,
) {
    let payload: SendVerified = match env.decode_payload() {
        Ok(p) => p,
        Err(e) => {
            reply_err(out_tx, id, ProtocolError::bad_request(e.to_string()));
            return;
        }
    };
    let session = match api::session_for_send(state, &payload.session_id) {
        Ok(s) => s,
        Err(e) => {
            reply_err(out_tx, id, e);
            return;
        }
    };
    let text = payload.text.clone();
    let session_for_blocking = Arc::clone(&session);
    let text_for_blocking = text.clone();
    let joined = tokio::task::spawn_blocking(move || {
        api::run_send_verified(&session_for_blocking, &text_for_blocking)
    })
    .await;
    let result = match joined {
        Ok(r) => r,
        Err(e) => {
            reply_err(out_tx, id, ProtocolError::internal(format!("verified submit task failed: {e}")));
            return;
        }
    };
    if attribute_concertmaster && result.submitted {
        api::record_concertmaster_steer(state, &payload.session_id, &text);
    }
    send(out_tx, &Envelope::message(id.to_string(), "session.send_verified", result));
}

/// Route an inbound binary frame (client -> daemon input/resize).
fn route_binary(bytes: &[u8], state: &AppState) {
    let frame = match decode_frame(bytes) {
        Ok(f) => f,
        Err(e) => {
            tracing::debug!(%e, "dropping malformed binary frame");
            return;
        }
    };
    let session = match state.sessions.get_bytes(&frame.session_id) {
        Some(s) => s,
        None => return,
    };
    match frame.kind {
        FrameKind::Input => {
            if let Err(err) = session.write_input(&frame.data) {
                tracing::debug!(%err, "failed to write input to pty");
            }
        }
        FrameKind::Resize => {
            if let Some((cols, rows)) = frame.as_resize() {
                let _ = session.resize(cols, rows);
            }
        }
        // Clients never send output frames.
        FrameKind::Output => {}
    }
}

/// Decode the payload as `P`, run the handler, and reply (result or structured
/// error) echoing the request's type and id. Shared by every simple verb.
fn handle_request<P, R, F>(env: &Envelope, out_tx: &UnboundedSender<Message>, id: &str, f: F)
where
    P: for<'de> serde::Deserialize<'de>,
    R: serde::Serialize,
    F: FnOnce(P) -> Result<R, ProtocolError>,
{
    let payload: P = match env.decode_payload() {
        Ok(p) => p,
        Err(e) => {
            reply_err(out_tx, id, ProtocolError::bad_request(e.to_string()));
            return;
        }
    };
    match f(payload) {
        Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), env.msg_type.clone(), resp)),
        Err(e) => {
            reply_err(out_tx, id, e);
        }
    }
}

/// Like [`handle_request`] but for a handler that must run asynchronously (e.g.
/// `daemon.lan.enable`, which binds a TCP listener). Decodes, awaits the handler, replies.
async fn handle_async_request<P, R, Fut, F>(
    env: &Envelope,
    out_tx: &UnboundedSender<Message>,
    id: &str,
    f: F,
) where
    P: for<'de> serde::Deserialize<'de>,
    R: serde::Serialize,
    Fut: std::future::Future<Output = Result<R, ProtocolError>>,
    F: FnOnce(P) -> Fut,
{
    let payload: P = match env.decode_payload() {
        Ok(p) => p,
        Err(e) => {
            reply_err(out_tx, id, ProtocolError::bad_request(e.to_string()));
            return;
        }
    };
    match f(payload).await {
        Ok(resp) => send(out_tx, &Envelope::message(id.to_string(), env.msg_type.clone(), resp)),
        Err(e) => {
            reply_err(out_tx, id, e);
        }
    }
}

/// Begin (or replace) this connection's card-event stream (`protocol.md` /
/// event.subscribe).
///
/// The subscription is taken *before* the catch-up query, so an event committed
/// during replay is never lost; the forwarder skips anything at or below the replay
/// high-water mark, so nothing is duplicated either. On broadcast lag the forwarder
/// re-reads from its cursor (the log is durable), so a slow client loses nothing.
fn start_event_stream(
    state: &AppState,
    out_tx: &UnboundedSender<Message>,
    id: &str,
    req: EventSubscribe,
    event_sub: &mut Option<JoinHandle<()>>,
) {
    let latest = match state.store.latest_event_cursor() {
        Ok(l) => l,
        Err(e) => {
            reply_err(out_tx, id, api::store_err(e));
            return;
        }
    };
    send(
        out_tx,
        &Envelope::message(
            id.to_string(),
            "event.subscribe",
            EventSubscribed { ok: true, latest_cursor: latest },
        ),
    );

    let store = Arc::clone(&state.store);
    let out = out_tx.clone();
    let handle = tokio::spawn(async move {
        // Subscribe first, then replay: the live tail overlaps the catch-up range
        // instead of racing it.
        let mut rx = store.subscribe_events();
        let mut last = req.cursor.unwrap_or_default();

        loop {
            let page = match store.events_after(
                if last.is_empty() { None } else { Some(&last) },
                EVENT_REPLAY_PAGE,
            ) {
                Ok(p) => p,
                Err(err) => {
                    tracing::warn!(%err, "event replay query failed; ending stream");
                    return;
                }
            };
            if page.is_empty() {
                break;
            }
            for event in page {
                last = event.id.clone();
                let env = Envelope::event("event.card_event", EventCardEvent { event });
                if out.send(text(&env)).is_err() {
                    return;
                }
            }
        }

        // Live tail: forward events newer than the replay high-water mark.
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if event.id <= last {
                        continue; // already delivered during catch-up
                    }
                    last = event.id.clone();
                    let env = Envelope::event("event.card_event", EventCardEvent { event });
                    if out.send(text(&env)).is_err() {
                        return;
                    }
                }
                Err(RecvError::Lagged(missed)) => {
                    // The log is durable: recover the gap by re-reading from the
                    // cursor instead of dropping events.
                    tracing::debug!(missed, "event subscriber lagged; re-reading from cursor");
                    loop {
                        let page = match store.events_after(Some(&last), EVENT_REPLAY_PAGE) {
                            Ok(p) => p,
                            Err(err) => {
                                tracing::warn!(%err, "event gap recovery failed; ending stream");
                                return;
                            }
                        };
                        if page.is_empty() {
                            break;
                        }
                        for event in page {
                            last = event.id.clone();
                            let env = Envelope::event("event.card_event", EventCardEvent { event });
                            if out.send(text(&env)).is_err() {
                                return;
                            }
                        }
                    }
                }
                Err(RecvError::Closed) => return,
            }
        }
    });

    if let Some(old) = event_sub.replace(handle) {
        old.abort();
    }
}

fn send(out_tx: &UnboundedSender<Message>, env: &Envelope) {
    let _ = out_tx.send(text(env));
}

fn reply_err(out_tx: &UnboundedSender<Message>, id: &str, err: ProtocolError) -> bool {
    send(out_tx, &Envelope::error(Some(id.to_string()), err));
    false
}

fn text(env: &Envelope) -> Message {
    match serde_json::to_string(env) {
        Ok(s) => Message::Text(s.into()),
        Err(e) => Message::Text(format!("{{\"v\":1,\"type\":\"error\",\"payload\":{{\"code\":\"internal\",\"message\":\"serialize failed: {e}\",\"retryable\":false}}}}").into()),
    }
}
