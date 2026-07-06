//! Session manager: spawns a command under a PTY, pumps its output into the
//! `ScreenModel` and a scrollback ring, broadcasts live output to attached
//! subscribers, and supports resize, kill (whole process tree), and list.
//!
//! Sessions are owned by the daemon and outlive client connections; detaching a
//! client never touches the child (`architecture.md` / two-process model).

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;
use ulid::Ulid;

use dflow_proto::{CursorPos, SessionInfo, StyledSnapshot};

use crate::job::KillGuard;
use crate::ring::ScrollbackRing;
use crate::screen::{repaint_ansi, AlacrittyScreen, ScreenModel};
use crate::store::{session_state, NewSession, Store};

/// Bytes of raw PTY output retained for reattach replay.
///
/// Sized generously (4 MiB) so a primary-screen session keeps a meaningful scrollback
/// history for scroll-up after a reconnect, not just the most recent window (Phase 2
/// reattach fix; `architecture.md` / reattach replays screen state + recent
/// scrollback). Full-screen (alt-screen) TUIs do not rely on the ring at all: their
/// replay is a snapshot repaint from the VT model. Per-session memory is bounded by
/// this cap; disk persistence with secret scrubbing is a later phase (`ring.rs`).
const RING_CAPACITY: usize = 4 * 1024 * 1024;
/// Broadcast backlog per session. A slow client that lags beyond this is handed a
/// fresh replay by the daemon rather than an unbounded buffer (`protocol.md`).
const BROADCAST_CAPACITY: usize = 2048;
/// PTY read chunk size.
const READ_CHUNK: usize = 8192;

/// Errors creating or driving a session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session command was empty")]
    EmptyCommand,
    #[error("pty error: {0}")]
    Pty(String),
    #[error("store error: {0}")]
    Store(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Everything needed to spawn a session.
///
/// The Phase 0 fields spawn the PTY; the Phase 1 fields (all optional) link the
/// session to a card so it is persisted to the store and reconciled on restart. A
/// spec with `card_id` and a `store` on the manager gets a `sessions` row inserted
/// before the reader loop starts, then a `session_ended` transition on exit.
#[derive(Debug, Clone, Default)]
pub struct SessionSpec {
    /// Adapter name recorded on the session (e.g. `"powershell"`, `"claude"`).
    pub harness: String,
    /// Resolved argv; `command[0]` is the program.
    pub command: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    /// Dispatch linkage: the card this session belongs to. `None` for a cardless
    /// (bare `session.create`) session, which still persists when a store is present.
    pub card_id: Option<Ulid>,
    /// Project linkage for a cardless session (cwd->project match); dispatch sessions
    /// derive their project from the card, but this is also set for them for symmetry.
    pub project_id: Option<String>,
    /// The leased worktree backing this session, recorded on the row.
    pub worktree_id: Option<Ulid>,
    /// The configured launcher this session was dispatched through (`agents.id`),
    /// recorded on the row when a launcher resolved (Phase 1.5).
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Preview of the first prompt, for the Projects view session list.
    pub first_prompt: Option<String>,
    /// Initial tab title (usually generated; kept null unless set).
    pub title: Option<String>,
    /// The predecessor session id when this session is a resume (`resumed_from`
    /// lineage chain, `architecture.md` / session resume).
    pub resumed_from: Option<String>,
    /// Directory under which this session's scrollback ring path is recorded.
    pub scrollback_dir: Option<PathBuf>,
}

/// Resolve a harness name to a default command for Phase 0.
///
/// Later phases resolve the full launch line from adapter manifests
/// (`adapters.md`); Phase 0 only needs a real interactive shell to prove the path.
pub fn default_command(harness: &str) -> Option<Vec<String>> {
    match harness {
        "powershell" | "pwsh" => Some(vec!["powershell.exe".into(), "-NoLogo".into()]),
        "cmd" => Some(vec!["cmd.exe".into()]),
        _ => None,
    }
}

/// Mutable session state guarded by one lock.
struct Inner {
    screen: Box<dyn ScreenModel>,
    ring: ScrollbackRing,
    alive: bool,
}

/// A live PTY session.
pub struct Session {
    pub id: Ulid,
    pub harness: String,
    /// The card this session is dispatched for, if any (dispatch sessions only).
    card_id: Option<Ulid>,
    /// The store, present for persisted dispatch sessions so exit finalizes the row.
    store: Option<Arc<Store>>,
    /// Ensures the exit transition (`session_ended`) is recorded exactly once.
    finalized: AtomicBool,
    inner: Mutex<Inner>,
    output_tx: broadcast::Sender<Arc<Vec<u8>>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    kill_guard: Mutex<KillGuard>,
    attached: AtomicUsize,
    created_at_ms: u64,
}

impl Session {
    /// Spawn a command under a new PTY and start pumping its output (store-less).
    pub fn spawn(spec: SessionSpec) -> Result<Arc<Session>, SessionError> {
        Self::spawn_with(spec, None)
    }

    /// Spawn a session, optionally persisting a `sessions` row via `store`.
    ///
    /// When `store` is set and `spec.card_id` is present, a row is inserted (with a
    /// `session_started` event) *before* the reader loop begins, so a fast-exiting
    /// child can never finalize a row that does not exist yet.
    pub fn spawn_with(
        spec: SessionSpec,
        store: Option<Arc<Store>>,
    ) -> Result<Arc<Session>, SessionError> {
        if spec.command.is_empty() {
            return Err(SessionError::EmptyCommand);
        }
        let id = Ulid::new();
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows: spec.rows, cols: spec.cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| SessionError::Pty(format!("openpty: {e}")))?;

        // Resolve the program to a launchable Windows form before spawning: a bare
        // manifest command like `codex`/`opencode`/`pi` otherwise resolves (inside
        // portable-pty) to an extension-less `#!/bin/sh` shim that `CreateProcessW`
        // rejects with os error 193; a `.cmd`/`.bat` shim is rewritten to run under
        // `cmd.exe /c` (`agents::launchable_command`; audit finding #2).
        let launch_argv = crate::agents::launchable_command(&spec.command);
        let mut cmd = CommandBuilder::new(&launch_argv[0]);
        for arg in &launch_argv[1..] {
            cmd.arg(arg);
        }
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SessionError::Pty(format!("spawn: {e}")))?;
        // Release the slave side; the child holds its own handle now.
        drop(pair.slave);

        let kill_guard = match child.process_id() {
            Some(pid) => KillGuard::attach_pid(pid),
            None => KillGuard::inert(),
        };

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SessionError::Pty(format!("take_writer: {e}")))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SessionError::Pty(format!("clone_reader: {e}")))?;

        let screen: Box<dyn ScreenModel> = Box::new(AlacrittyScreen::new(spec.cols, spec.rows));
        let (output_tx, _rx) = broadcast::channel(BROADCAST_CAPACITY);

        let session = Arc::new(Session {
            id,
            harness: spec.harness.clone(),
            card_id: spec.card_id,
            store: store.clone(),
            finalized: AtomicBool::new(false),
            inner: Mutex::new(Inner {
                screen,
                ring: ScrollbackRing::new(RING_CAPACITY),
                alive: true,
            }),
            output_tx,
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            child: Mutex::new(child),
            kill_guard: Mutex::new(kill_guard),
            attached: AtomicUsize::new(0),
            created_at_ms: now_ms(),
        });

        // Persist the row before the reader can observe EOF, whenever a store is
        // present. Both dispatch (carded) and bare (cardless) sessions persist now, so
        // a cardless session survives a daemon restart with its Projects-tree identity
        // (`data-model.md` session-first note; Phase 2 API reconciliation).
        if let Some(store) = &store {
            let scrollback_path = spec
                .scrollback_dir
                .as_ref()
                .map(|d| d.join(format!("{id}.ring")).to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("{id}.ring"));
            if let Err(err) = store.create_session(NewSession {
                id: id.to_string(),
                card_id: spec.card_id.map(|c| c.to_string()),
                project_id: spec.project_id.clone(),
                cwd: spec.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
                harness: spec.harness.clone(),
                model: spec.model.clone(),
                effort: spec.effort.clone(),
                state: session_state::WORKING.to_string(),
                worktree_id: spec.worktree_id.map(|w| w.to_string()),
                scrollback_path,
                first_prompt: spec.first_prompt.clone(),
                resumed_from: spec.resumed_from.clone(),
                title: spec.title.clone(),
                agent_id: spec.agent_id.clone(),
            }) {
                // A failed insert would leave an unpersisted, unreconcilable session;
                // fail the spawn cleanly rather than run a ghost.
                session.kill();
                return Err(SessionError::Store(err.to_string()));
            }
        }

        let reader_session = Arc::clone(&session);
        thread::Builder::new()
            .name(format!("pty-reader-{id}"))
            .spawn(move || pump_reader(reader, reader_session))?;

        // Watch the direct child so the session self-terminates when the agent CLI exits,
        // even if a ConPTY descendant keeps the pty open (EOF never arrives) (`finding #5`).
        let watch_session = Arc::clone(&session);
        thread::Builder::new()
            .name(format!("pty-watch-{id}"))
            .spawn(move || watch_child(watch_session))?;

        Ok(session)
    }

    /// The card this session belongs to, if it is a dispatch session.
    pub fn card_id(&self) -> Option<Ulid> {
        self.card_id
    }

    /// Record the session's exit transition exactly once (persisted sessions only).
    /// Every store-backed session persists a row now (carded or cardless), so the
    /// finalize runs whenever a store is present. `note` carries the child's exit code
    /// when the session self-terminated (`finding #5`), else `None`.
    fn finalize_exit(&self, note: Option<String>) {
        if self.finalized.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(store) = &self.store {
            if let Err(err) = store.finalize_session_note(
                &self.id.to_string(),
                session_state::DONE,
                note.as_deref(),
            ) {
                tracing::debug!(session_id = %self.id, %err, "could not finalize session row");
            }
        }
    }

    /// Tear down the whole process tree (the per-session Job Object reaps every
    /// descendant, then the direct child is killed) and mark the session not-alive.
    /// Does NOT finalize the row; callers pair it with [`Session::finalize_exit`].
    fn teardown_tree(&self) {
        self.kill_guard.lock().expect("kill guard poisoned").kill();
        let _ = self.child.lock().expect("session child poisoned").kill();
        self.inner.lock().expect("session inner poisoned").alive = false;
    }

    /// The 16-byte form of the session id used in binary PTY frames.
    pub fn id_bytes(&self) -> [u8; 16] {
        self.id.to_bytes()
    }

    /// Attach: return the replay bytes and a receiver for live output. The replay and
    /// subscription are taken together under the inner lock so the handoff is lossless
    /// and duplicate-free relative to the reader.
    ///
    /// The replay reconstructs the CURRENT screen from the VT model, not raw ring bytes
    /// (Phase 2 reattach fix): an alt-screen TUI gets a snapshot repaint alone (its
    /// chrome scrolled out of the ring long ago); a primary-screen session gets ring
    /// scrollback history followed by a snapshot repaint, so both history and the
    /// current screen are correct, with terminal modes restored so scrolling and arrow
    /// keys keep working.
    pub fn attach(&self) -> (Vec<u8>, broadcast::Receiver<Arc<Vec<u8>>>) {
        let handoff = {
            let inner = self.inner.lock().expect("session inner poisoned");
            let rx = self.output_tx.subscribe();
            let replay = compose_replay(&inner);
            (replay, rx)
        };
        self.attached.fetch_add(1, Ordering::SeqCst);
        handoff
    }

    /// The current reattach replay payload without subscribing (used to recover a
    /// lagged client with a correct, mode-aware repaint rather than raw ring bytes).
    pub fn repaint_payload(&self) -> Vec<u8> {
        let inner = self.inner.lock().expect("session inner poisoned");
        compose_replay(&inner)
    }

    /// Note that a client detached. Never affects the child process.
    pub fn mark_detached(&self) {
        let _ = self
            .attached
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| Some(v.saturating_sub(1)));
    }

    /// A styled snapshot of the visible screen (for the attach response).
    pub fn styled_snapshot(&self) -> StyledSnapshot {
        self.inner.lock().expect("session inner poisoned").screen.styled_snapshot()
    }

    /// The current cursor position.
    pub fn cursor(&self) -> CursorPos {
        self.inner.lock().expect("session inner poisoned").screen.cursor()
    }

    /// Current `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        self.inner.lock().expect("session inner poisoned").screen.size()
    }

    /// The visible screen as plain text (diagnostics/tests).
    pub fn capture_plain(&self) -> String {
        self.inner.lock().expect("session inner poisoned").screen.capture_plain()
    }

    /// A read-only `session.peek`: the last `max_lines` lines of the visible screen as
    /// plain text, with known secret values redacted (`security.md`: captures that leave
    /// the session are scrubbed). This reads the existing screen model and never resizes
    /// the PTY, so a human attached to the same session sees no repaint jiggle
    /// (`phase6-mcp.md` merge-time request 3).
    pub fn peek_scrubbed(&self, max_lines: usize, secrets: &[String]) -> String {
        let plain = self.capture_plain();
        let lines: Vec<&str> = plain.lines().collect();
        let start = lines.len().saturating_sub(max_lines.max(1));
        let tail = lines[start..].join("\n");
        crate::secret::scrub(&tail, secrets)
    }

    /// The full scrollback plus visible screen as plain text (diagnostics/tests).
    pub fn capture_scrollback(&self) -> String {
        self.inner.lock().expect("session inner poisoned").screen.capture_scrollback()
    }

    /// The current replay bytes without subscribing (used to recover a lagged client).
    pub fn replay_bytes(&self) -> Vec<u8> {
        self.inner.lock().expect("session inner poisoned").ring.snapshot()
    }

    /// Write raw bytes to the PTY (keystrokes/input).
    pub fn write_input(&self, bytes: &[u8]) -> io::Result<()> {
        let mut writer = self.writer.lock().expect("session writer poisoned");
        writer.write_all(bytes)?;
        writer.flush()
    }

    /// Resize the PTY and the screen model together.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.master
            .lock()
            .expect("session master poisoned")
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| SessionError::Pty(format!("resize: {e}")))?;
        self.inner.lock().expect("session inner poisoned").screen.resize(cols, rows);
        Ok(())
    }

    /// Whether the child process is still running.
    pub fn is_alive(&self) -> bool {
        self.inner.lock().expect("session inner poisoned").alive
    }

    /// Terminate the session and its whole process tree.
    pub fn kill(&self) {
        self.teardown_tree();
        self.finalize_exit(None);
    }

    /// Graceful-shutdown teardown: mark the persisted row `interrupted` (not `done`) so
    /// the resume path picks it up on the next daemon start (`architecture.md` / daemon
    /// restarts and session resume), then kill the whole process tree via the Job
    /// Object. Distinct from `kill` (a user-initiated kill is `done`, a daemon shutdown
    /// is `interrupted`).
    pub fn shutdown_interrupted(&self) {
        if !self.finalized.swap(true, Ordering::SeqCst) {
            if let Some(store) = &self.store {
                if let Err(err) =
                    store.finalize_session(&self.id.to_string(), session_state::INTERRUPTED)
                {
                    tracing::debug!(session_id = %self.id, %err, "could not mark session interrupted");
                }
            }
        }
        self.kill_guard.lock().expect("kill guard poisoned").kill();
        let _ = self.child.lock().expect("session child poisoned").kill();
        self.inner.lock().expect("session inner poisoned").alive = false;
    }

    /// A compact fleet-table row.
    pub fn info(&self) -> SessionInfo {
        let (cols, rows, alive) = {
            let inner = self.inner.lock().expect("session inner poisoned");
            let (c, r) = inner.screen.size();
            (c, r, inner.alive)
        };
        SessionInfo {
            session_id: self.id.to_string(),
            harness: self.harness.clone(),
            cols,
            rows,
            alive,
            attached: self.attached.load(Ordering::SeqCst),
            created_at_ms: self.created_at_ms,
        }
    }
}

/// Compose the reattach replay from a locked session inner (Phase 2 reattach fix).
///
/// Alt-screen: a snapshot repaint alone. Primary screen: ring scrollback history then
/// a snapshot repaint. In both cases the repaint preamble restores terminal modes so a
/// freshly connected client behaves like the live TUI.
fn compose_replay(inner: &Inner) -> Vec<u8> {
    let snapshot = inner.screen.styled_snapshot();
    let cursor = inner.screen.cursor();
    let modes = inner.screen.terminal_modes();
    let repaint = repaint_ansi(&snapshot, &cursor, &modes);
    if inner.screen.is_alt_screen() {
        repaint
    } else {
        let mut replay = inner.ring.snapshot();
        replay.extend_from_slice(&repaint);
        replay
    }
}

/// The blocking PTY read loop: feed the screen model, append to the ring, and
/// broadcast to subscribers, all under the inner lock so `attach()` is lossless.
fn pump_reader(mut reader: Box<dyn Read + Send>, session: Arc<Session>) {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                let responses = {
                    let mut inner = match session.inner.lock() {
                        Ok(g) => g,
                        Err(_) => break,
                    };
                    inner.screen.feed(chunk);
                    inner.ring.push(chunk);
                    // Send while holding the inner lock so attach() gets a lossless,
                    // duplicate-free handoff between replay and live frames.
                    let _ = session.output_tx.send(Arc::new(chunk.to_vec()));
                    inner.screen.take_responses()
                };
                // Feed terminal query responses (DSR/DA) back to the PTY so ConPTY
                // and the shell make progress. Done outside the inner lock.
                if !responses.is_empty() {
                    if let Err(err) = session.write_input(&responses) {
                        tracing::debug!(session_id = %session.id, %err, "failed to write terminal responses");
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if let Ok(mut inner) = session.inner.lock() {
        inner.alive = false;
    }
    // The pty reached EOF (every client, including any lingering descendant, is gone);
    // record the terminal transition (once). The child-exit watcher may have finalized
    // first with the exit code; whichever runs first wins (finalize is idempotent).
    session.finalize_exit(None);
    tracing::debug!(session_id = %session.id, "pty reader loop ended");
}

/// How long to wait after the DIRECT child exits before finalizing the session, so the
/// pty reader can drain the child's final output first (`finding #5`). Overridable via
/// `DFLOW_CHILD_EXIT_GRACE_MS` for ops tuning and tests.
fn child_exit_grace() -> std::time::Duration {
    std::env::var("DFLOW_CHILD_EXIT_GRACE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(std::time::Duration::from_millis(500))
}

/// Poll interval for the direct-child exit watcher.
const CHILD_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

/// Watch the DIRECT child process and finalize the session when it exits, even when a
/// ConPTY descendant keeps the pty open so the reader never sees EOF (`finding #5`: a real
/// agent CLI can exit while a lingering grandchild holds the pseudo-console, so relying on
/// the reader's EOF alone leaves the session "alive" until an external timeout force-kills
/// it - which killed a gate fixer mid-work in the live audit).
///
/// On exit: a short grace period to flush final output, then tear down the whole tree via
/// the per-session Job Object (which reaps the lingering descendant and, once the last pty
/// client is gone, lets the reader reach EOF), and finalize the row `done` with the child's
/// exit code. The daemon-wide reaping job (`job.rs`) remains the outer safety net.
fn watch_child(session: Arc<Session>) {
    loop {
        let status = {
            let mut child = match session.child.lock() {
                Ok(c) => c,
                Err(_) => return,
            };
            match child.try_wait() {
                Ok(status) => status,
                // Cannot observe the child; leave finalization to the reader/kill paths.
                Err(_) => return,
            }
        };
        match status {
            Some(status) => {
                // Let the reader drain any last output before we tear the pty down.
                thread::sleep(child_exit_grace());
                session.teardown_tree();
                session.finalize_exit(Some(format!("exited (code {})", status.exit_code())));
                return;
            }
            None => {
                // Already finalized elsewhere (explicit kill / shutdown): stop watching.
                if !session.is_alive() {
                    return;
                }
                thread::sleep(CHILD_POLL_INTERVAL);
            }
        }
    }
}

/// Owns all live sessions.
#[derive(Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<Ulid, Arc<Session>>>,
    /// When set, dispatch sessions (those with a `card_id`) persist to the store.
    store: Option<Arc<Store>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// A session manager that persists dispatch sessions to `store`.
    pub fn with_store(store: Arc<Store>) -> Self {
        SessionManager { sessions: Mutex::new(HashMap::new()), store: Some(store) }
    }

    /// Spawn and register a session, persisting it when it is a dispatch session.
    pub fn create(&self, spec: SessionSpec) -> Result<Arc<Session>, SessionError> {
        let session = Session::spawn_with(spec, self.store.clone())?;
        self.sessions.lock().expect("sessions poisoned").insert(session.id, Arc::clone(&session));
        Ok(session)
    }

    /// Live sessions linked to a card (dispatch sessions).
    pub fn dispatch_sessions(&self) -> Vec<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .values()
            .filter(|s| s.card_id().is_some())
            .cloned()
            .collect()
    }

    /// Every live session, for the supervision loop (`adapters.md` tier 3). Both
    /// carded and cardless persisted sessions are supervised; the supervisor skips any
    /// without a store row.
    pub fn live_sessions(&self) -> Vec<Arc<Session>> {
        self.sessions.lock().expect("sessions poisoned").values().cloned().collect()
    }

    /// Live sessions linked to a specific card.
    pub fn sessions_for_card(&self, card_id: &Ulid) -> Vec<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .values()
            .filter(|s| s.card_id().as_ref() == Some(card_id))
            .cloned()
            .collect()
    }

    /// Look up a session by ULID.
    pub fn get(&self, id: &Ulid) -> Option<Arc<Session>> {
        self.sessions.lock().expect("sessions poisoned").get(id).cloned()
    }

    /// Look up a session by its string ULID.
    pub fn get_str(&self, id: &str) -> Option<Arc<Session>> {
        Ulid::from_string(id).ok().and_then(|u| self.get(&u))
    }

    /// Look up a session by the 16-byte id from a binary frame.
    pub fn get_bytes(&self, id: &[u8; 16]) -> Option<Arc<Session>> {
        self.get(&Ulid::from_bytes(*id))
    }

    /// Compact fleet table, oldest first.
    pub fn list(&self) -> Vec<SessionInfo> {
        let map = self.sessions.lock().expect("sessions poisoned");
        let mut infos: Vec<SessionInfo> = map.values().map(|s| s.info()).collect();
        infos.sort_by_key(|i| i.created_at_ms);
        infos
    }

    /// Number of registered sessions.
    pub fn count(&self) -> usize {
        self.sessions.lock().expect("sessions poisoned").len()
    }

    /// Kill a session's process tree and remove it from the manager.
    pub fn kill(&self, id: &Ulid) -> bool {
        let removed = self.sessions.lock().expect("sessions poisoned").remove(id);
        match removed {
            Some(session) => {
                session.kill();
                true
            }
            None => false,
        }
    }

    /// Kill every session (hard teardown; each finalizes as `done`).
    pub fn shutdown_all(&self) {
        let sessions: Vec<Arc<Session>> =
            self.sessions.lock().expect("sessions poisoned").drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.kill();
        }
    }

    /// Graceful daemon shutdown: mark every persisted session `interrupted` (resumable)
    /// and kill its process tree, so no agent CLI is ever orphaned and resume works on
    /// the next start (`architecture.md`).
    pub fn shutdown_all_interrupted(&self) {
        let sessions: Vec<Arc<Session>> =
            self.sessions.lock().expect("sessions poisoned").drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.shutdown_interrupted();
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn wait_until<F: Fn() -> bool>(pred: F, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if pred() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        pred()
    }

    /// Type `input` and wait until `seen()` observes its effect, re-sending every two
    /// seconds. A busy machine can swallow keystrokes typed while the shell (or a
    /// plugin like Clink) is still initializing even though banner output already
    /// appeared, so a single fixed send is a flake; retrying converts the lost-input
    /// race into a bounded retry, the same posture as verified submit.
    fn send_until<F: Fn() -> bool>(session: &Session, input: &[u8], seen: F, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            session.write_input(input).expect("write input");
            let attempt_deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < attempt_deadline {
                if seen() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        seen()
    }

    fn interactive_cmd() -> SessionSpec {
        SessionSpec {
            harness: "cmd".into(),
            // /D skips registry AutoRun commands: a user's shell customization (e.g.
            // Clink's injected line editor and prompt) must never decide whether these
            // tests can type into the PTY.
            command: vec!["cmd.exe".into(), "/D".into()],
            cols: 80,
            rows: 24,
            cwd: None,
            env: BTreeMap::new(),
            ..Default::default()
        }
    }

    /// Drive an interactive shell and confirm the screen model captures its output.
    /// This exercises the real path: PTY spawn, the DSR handshake, input, and the
    /// screen model, without needing an extra test binary.
    #[test]
    fn session_captures_child_output() {
        let mgr = SessionManager::new();
        let session = mgr.create(interactive_cmd()).expect("spawn session");

        // Wait for the shell to finish its startup handshake and draw a prompt.
        assert!(
            wait_until(|| !session.capture_scrollback().trim().is_empty(), Duration::from_secs(10)),
            "shell never produced any output"
        );

        let captured = send_until(
            &session,
            b"echo DAPPERFLOW_MARKER\r\n",
            || session.capture_scrollback().contains("DAPPERFLOW_MARKER"),
            Duration::from_secs(15),
        );
        assert!(captured, "expected marker in scrollback, got: {:?}", session.capture_scrollback());
        session.kill();
    }

    /// A new attach after output was produced must replay that output. This is the
    /// wire-level persistence primitive: reattach replays scrollback.
    #[test]
    fn attach_replays_prior_output() {
        let mgr = SessionManager::new();
        let session = mgr.create(interactive_cmd()).expect("spawn session");

        assert!(
            wait_until(|| !session.replay_bytes().is_empty(), Duration::from_secs(10)),
            "shell never produced any output"
        );
        assert!(
            send_until(
                &session,
                b"echo REPLAY_ME\r\n",
                || String::from_utf8_lossy(&session.replay_bytes()).contains("REPLAY_ME"),
                Duration::from_secs(15)
            ),
            "child output never reached the ring"
        );

        // Attach after the fact: the replay must contain the prior output, exactly
        // as a reopened GUI would reconstruct it.
        let (replay, _rx) = session.attach();
        let text = String::from_utf8_lossy(&replay);
        assert!(text.contains("REPLAY_ME"), "attach replay missing prior output: {text:?}");
        session.kill();
    }

    /// `session.peek` is bounded, redacts known secret values, and never resizes the
    /// PTY (`phase6-mcp.md` merge-time request 3; `security.md` / captures that leave the
    /// session are scrubbed).
    #[test]
    fn peek_scrubbed_is_bounded_redacted_and_does_not_resize() {
        let mgr = SessionManager::new();
        let session = mgr.create(interactive_cmd()).expect("spawn session");
        assert!(
            wait_until(|| !session.capture_scrollback().trim().is_empty(), Duration::from_secs(10)),
            "shell never produced any output"
        );
        let size_before = session.size();

        let secret = "SEEKRIT-abcdef123456";
        assert!(
            send_until(
                &session,
                format!("echo VAL={secret}\r\n").as_bytes(),
                || session.capture_scrollback().contains(secret),
                Duration::from_secs(15),
            ),
            "shell never echoed the secret"
        );

        // A generous window covers the whole visible screen: the secret must be redacted.
        let full = session.peek_scrubbed(200, &[secret.to_string()]);
        assert!(!full.contains(secret), "peek must redact the secret value: {full:?}");
        assert!(full.contains("[dflow:redacted]"), "expected the redaction marker: {full:?}");

        // A tight window is bounded to the requested number of lines.
        let bounded = session.peek_scrubbed(3, &[secret.to_string()]);
        assert!(bounded.lines().count() <= 3, "peek must be bounded: got {}", bounded.lines().count());

        // The peek read the screen model only - the PTY size is unchanged (no jiggle).
        assert_eq!(session.size(), size_before, "peek must not resize the PTY");
        session.kill();
    }

    #[test]
    fn list_and_kill_track_sessions() {
        let spec = SessionSpec {
            harness: "cmd".into(),
            command: vec!["cmd.exe".into()],
            cols: 80,
            rows: 24,
            cwd: None,
            env: BTreeMap::new(),
            ..Default::default()
        };
        let mgr = SessionManager::new();
        let session = mgr.create(spec).expect("spawn session");
        assert_eq!(mgr.count(), 1);
        assert_eq!(mgr.list().len(), 1);

        assert!(mgr.kill(&session.id));
        assert_eq!(mgr.count(), 0);
        assert!(!mgr.kill(&session.id), "killing an unknown session returns false");
    }

    /// `finding #5`: a session whose direct child exits while a ConPTY descendant lingers
    /// must SELF-terminate within the grace window (reader EOF never arrives on its own),
    /// finalize the row `done` with the child's exit code, and reap the descendant. Before
    /// the child-exit watcher, such a session stayed "alive" until an external timeout
    /// force-killed it (which killed a gate fixer mid-work in the live audit).
    #[cfg(windows)]
    #[test]
    fn session_self_terminates_when_child_exits_with_lingering_descendant() {
        let stamp =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("dflow-finding5-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("descendant.pid");
        let parent_ps1 = dir.join("parent.ps1");
        // The direct child launches a DETACHED `ping` in the same console (so it holds the
        // pty open after the parent exits), records the descendant's pid so the test can
        // confirm it was reaped, then exits. `ping -n 120` lingers far past the test window.
        let script = format!(
            "$p = Start-Process -NoNewWindow -FilePath 'ping' -ArgumentList @('-n','120','127.0.0.1') -PassThru\n\
             $p.Id | Out-File -FilePath '{}' -Encoding ascii\n\
             Start-Sleep -Milliseconds 400\n",
            pidfile.display()
        );
        std::fs::write(&parent_ps1, script).unwrap();

        let store = Arc::new(Store::open_in_memory().unwrap());
        let mgr = SessionManager::with_store(Arc::clone(&store));
        let spec = SessionSpec {
            harness: "powershell".into(),
            command: vec![
                "powershell.exe".into(),
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                parent_ps1.to_string_lossy().into_owned(),
            ],
            cols: 80,
            rows: 24,
            scrollback_dir: Some(dir.clone()),
            ..Default::default()
        };
        let session = mgr.create(spec).expect("spawn session");
        let sid = session.id.to_string();

        // The descendant started and the parent recorded its pid (proof it holds the pty).
        assert!(
            wait_until(|| pidfile.exists(), Duration::from_secs(20)),
            "parent never launched the lingering descendant"
        );
        let pid: u32 =
            std::fs::read_to_string(&pidfile).unwrap().trim().parse().expect("descendant pid");

        // The session must self-finalize shortly after its direct child exits - within the
        // grace window plus slack, NOT hang until an external timeout. Without the watcher
        // this never flips because the lingering `ping` keeps the pty from reaching EOF.
        assert!(
            wait_until(|| !session.is_alive(), Duration::from_secs(15)),
            "session did not self-terminate after its child exited"
        );

        // The row is finalized terminal, `ended_at` stamped, exit code recorded.
        let row = store.get_session(&sid).unwrap().expect("session row");
        assert!(session_state::is_terminal(&row.state), "row not terminal: {}", row.state);
        assert!(row.ended_at.is_some(), "ended_at not stamped");
        assert!(
            row.status_note.as_deref().unwrap_or_default().contains("exited"),
            "exit code note missing: {:?}",
            row.status_note
        );

        // The lingering descendant was reaped by the per-session Job Object.
        assert!(
            wait_until(|| !pid_is_alive(pid), Duration::from_secs(15)),
            "lingering descendant {pid} was not reaped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whether a pid is a still-running process (no unsafe process-table scan, no kill).
    #[cfg(windows)]
    fn pid_is_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: OpenProcess returns a handle we own (or null); it is closed exactly once.
        // GetExitCodeProcess reads the exit status into `code`.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }
}
