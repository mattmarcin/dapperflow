//! Tier-2 native signals over a loopback HTTP hook endpoint (`adapters.md` / tier 2;
//! `spike2-harness-signal-audit.md`, claude recommendation).
//!
//! Claude Code is the flagship: its Stop / Notification / SessionEnd hooks POST JSON
//! to a URL, and its `Notification` type distinguishes `agent_completed` from
//! `agent_needs_input` from `permission_prompt` from `idle_prompt` without screen
//! scraping. At dispatch/session-create for a claude-family launcher the daemon
//! materializes a session-scoped `--settings` file wiring those hooks to
//! `http://127.0.0.1:<port>/hooks/<token>` - never touching the user's global or
//! project settings (`--settings` merges additively, verified against the docs). The
//! per-session token in the URL scopes each POST to one session.
//!
//! Every hook event delivers the harness-native `session_id`, captured into
//! `resume_ref` (latest-wins) so daemon-restart resume works even after a crash
//! (`adapters.md` / Resume-ref capture).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use dflow_core::{bundled_manifests, session_state};
use serde::Deserialize;

use crate::api;
use crate::server::AppState;

/// Maps a per-session hook token to the daemon session ULID it scopes.
#[derive(Default)]
pub struct HookRegistry {
    map: Mutex<HashMap<String, String>>,
}

impl HookRegistry {
    /// Register `token -> session_id` (called right after the session spawns).
    pub fn register(&self, token: String, session_id: String) {
        self.map.lock().expect("hook registry poisoned").insert(token, session_id);
    }

    /// Resolve a token to its session id.
    pub fn resolve(&self, token: &str) -> Option<String> {
        self.map.lock().expect("hook registry poisoned").get(token).cloned()
    }

    /// Drop every token bound to a session (called when the session ends).
    pub fn unregister_session(&self, session_id: &str) {
        self.map.lock().expect("hook registry poisoned").retain(|_, sid| sid != session_id);
    }
}

/// The subset of a Claude Code hook payload the daemon consumes (`spike2` / claude
/// hooks). All fields optional so a shape change never rejects the POST.
#[derive(Debug, Default, Deserialize)]
pub struct HookEvent {
    /// The harness-native session id, captured into `resume_ref`.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub hook_event_name: Option<String>,
    /// For `Notification`: `agent_completed | agent_needs_input | permission_prompt |
    /// idle_prompt | elicitation_dialog | ...`.
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// For `SessionEnd`: the end reason.
    #[serde(default)]
    pub reason: Option<String>,
}

/// The loopback hook endpoint: `POST /hooks/{token}` with the hook event JSON body.
/// Always answers 200 with a JSON body so a hook never blocks the agent (Claude Code
/// rejects a non-JSON HTTP-hook response with a visible error, and a non-2xx is a
/// non-blocking error); an unknown token is logged and ignored.
pub async fn hook_handler(
    Path(token): Path<String>,
    State(state): State<AppState>,
    body: Bytes,
) -> (StatusCode, Json<serde_json::Value>) {
    let event: HookEvent = serde_json::from_slice(&body).unwrap_or_default();
    // Log the raw payload so the Phase 2 live proof can capture real hook traffic.
    tracing::info!(
        event = %event.hook_event_name.as_deref().unwrap_or("?"),
        notification = %event.notification_type.as_deref().unwrap_or(""),
        native_session = %event.session_id.as_deref().unwrap_or(""),
        cwd = %event.cwd.as_deref().unwrap_or(""),
        transcript = %event.transcript_path.as_deref().unwrap_or(""),
        message = %event.message.as_deref().unwrap_or(""),
        raw = %String::from_utf8_lossy(&body),
        "hook POST received"
    );
    match state.hooks.resolve(&token) {
        Some(session_id) => apply_hook_event(&state, &session_id, &event),
        None => tracing::debug!("hook POST with unknown/expired token"),
    }
    // An empty JSON object is Claude Code's "no special action, continue" response.
    (StatusCode::OK, Json(serde_json::json!({})))
}

/// Map a hook event to a lifecycle transition and capture the resume ref
/// (`adapters.md` / three-tier signal model, tier 2).
fn apply_hook_event(state: &AppState, session_id: &str, event: &HookEvent) {
    let store = &state.store;

    // Capture the harness-native session id into resume_ref (latest-wins by write
    // order): a resumed claude session reassigns its id, so re-capture from the new
    // session's first hook event guards against stale ids after /exit.
    if let Some(native) = event.session_id.as_deref().filter(|s| !s.is_empty()) {
        let _ = store.set_resume_ref(session_id, native);
    }

    let row = match store.get_session(session_id) {
        Ok(Some(row)) => row,
        _ => return,
    };
    if session_state::is_terminal(&row.state) {
        return;
    }

    match event.hook_event_name.as_deref().unwrap_or("") {
        "Notification" => match event.notification_type.as_deref().unwrap_or("") {
            // A permission gate is a trust/permission dialog; a plain needs-input or an
            // elicitation is the agent blocked awaiting a decision (deliverable 7).
            "permission_prompt" => set_needs_input(state, session_id, row.card_id.as_deref(), "trust_dialog"),
            "agent_needs_input" | "elicitation_dialog" => {
                set_needs_input(state, session_id, row.card_id.as_deref(), "agent_blocked")
            }
            // Turn complete or gone idle: the agent is idle, not blocked.
            "agent_completed" | "idle_prompt" => set_idle(state, session_id, &row.state),
            _ => {}
        },
        // Stop fires when the agent finishes responding: the turn ended, go idle.
        "Stop" | "SubagentStop" => set_idle(state, session_id, &row.state),
        // SessionEnd: resume_ref already captured; the PTY EOF drives the DONE finalize.
        "SessionEnd" => {
            tracing::info!(session_id, reason = %event.reason.as_deref().unwrap_or(""), "hook SessionEnd");
        }
        _ => {}
    }
}

/// Transition to `needs_input` with the given kind, if not already there.
fn set_needs_input(state: &AppState, session_id: &str, card_id: Option<&str>, kind: &str) {
    let score = api::needs_input_score(state, card_id);
    if let Err(err) = state.store.mark_session_needs_input(session_id, kind, score) {
        tracing::debug!(%err, session_id, "hook needs_input transition failed");
    }
}

/// Transition to `idle` (turn complete). Clears a prior needs_input (resolving its
/// Needs You item); otherwise sets idle unless already idle.
fn set_idle(state: &AppState, session_id: &str, current: &str) {
    let result = if current == session_state::NEEDS_INPUT {
        state.store.clear_session_needs_input(session_id, session_state::IDLE)
    } else if current != session_state::IDLE {
        state.store.set_session_state(session_id, session_state::IDLE)
    } else {
        Ok(())
    };
    if let Err(err) = result {
        tracing::debug!(%err, session_id, "hook idle transition failed");
    }
}

/// If `harness` has a native HTTP-hook tier-2 channel (claude), materialize a
/// session-scoped `--settings` file wiring Stop/Notification/SessionEnd to the daemon
/// and return `(argv_with_settings, Some(token))`. Otherwise return the argv unchanged
/// and `None`. Register `token -> session_id` after the session spawns.
///
/// The settings file is additive (Claude merges `--settings` on top of its normal
/// sources), so the user's global `~/.claude/settings.json` and any project settings
/// are never touched.
pub fn wire_native_hooks(state: &AppState, harness: &str, mut argv: Vec<String>) -> (Vec<String>, Option<String>) {
    let native = bundled_manifests().get(harness).map(|m| m.signals.native.as_str());
    if native != Some("hooks") {
        return (argv, None);
    }
    let port = state.http_port.load(Ordering::SeqCst);
    if port == 0 {
        tracing::warn!("hook endpoint port not ready; launching claude without hooks");
        return (argv, None);
    }
    let token = mint_token();
    let url = format!("http://127.0.0.1:{port}/hooks/{token}");
    let settings = hook_settings_json(&url);
    let path = state.data_dir.hooks_dir().join(format!("{token}.json"));
    if let Err(err) = std::fs::write(&path, settings) {
        tracing::warn!(%err, "could not write hook settings file; launching without hooks");
        return (argv, None);
    }
    // Insert `--settings <path>` right after the command so the positional brief (last)
    // and any launcher extra_args are untouched.
    let settings_path = path.to_string_lossy().into_owned();
    let insert_at = argv.len().min(1);
    argv.insert(insert_at, settings_path);
    argv.insert(insert_at, "--settings".to_string());
    (argv, Some(token))
}

/// If `harness` uses the codex `notify` tier-2 mechanism, inject a `-c notify` override
/// pointing at the `dflow notify-forward` bridge, so an `agent-turn-complete` fires the
/// bridge (which forwards over the per-task token) without touching the user's global
/// `~/.codex/config.toml` (`adapters.md` / codex row; `phase2-signals.md` deferred
/// bridge). Returns the argv unchanged for any other harness.
///
/// codex invokes the notify program with the JSON payload appended as the final
/// argument, so the resolved argv is effectively `dflow notify-forward '<json>'`.
pub fn wire_codex_notify(harness: &str, mut argv: Vec<String>) -> Vec<String> {
    let native = bundled_manifests().get(harness).map(|m| m.signals.native.as_str());
    if native != Some("notify") {
        return argv;
    }
    let dflow = crate::tokens::dflow_binary_path()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "dflow".to_string());
    let value = format!("notify={}", toml_string_array(&[&dflow, "notify-forward"]));
    // Insert `-c notify=[...]` right after the command so the positional prompt (last)
    // and any launcher extra_args are untouched.
    let insert_at = argv.len().min(1);
    argv.insert(insert_at, value);
    argv.insert(insert_at, "-c".to_string());
    argv
}

/// Render a TOML array-of-strings literal (`["a","b"]`) with basic-string escaping, for
/// a codex `-c` override value. Windows paths (backslashes) escape correctly here.
fn toml_string_array(items: &[&str]) -> String {
    let parts: Vec<String> = items
        .iter()
        .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", parts.join(","))
}

/// Apply a forwarded codex `notify` payload (`dflow notify-forward` -> `notify.forward`):
/// capture `thread-id` into `resume_ref` and, on `agent-turn-complete`, mark the session
/// idle (turn ended). Permissive: an unparseable payload is logged and ignored.
pub fn apply_codex_notify(state: &AppState, session_id: &str, payload: &str) {
    let value: serde_json::Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(%err, "codex notify payload not JSON; ignoring");
            return;
        }
    };
    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    // codex has used both `thread-id`/`thread_id` and `session-id` across versions.
    let thread = ["thread-id", "thread_id", "session-id", "session_id"]
        .iter()
        .find_map(|k| value.get(*k).and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty());
    tracing::info!(session_id, kind, thread = %thread.unwrap_or(""), "codex notify forwarded");
    if let Some(t) = thread {
        let _ = state.store.set_resume_ref(session_id, t);
    }
    let row = match state.store.get_session(session_id) {
        Ok(Some(row)) => row,
        _ => return,
    };
    if session_state::is_terminal(&row.state) {
        return;
    }
    if kind == "agent-turn-complete" {
        set_idle(state, session_id, &row.state);
    }
}

/// The Claude Code settings JSON wiring the three lifecycle hooks to `url` over HTTP.
fn hook_settings_json(url: &str) -> String {
    serde_json::json!({
        "hooks": {
            "Stop": [ { "hooks": [ { "type": "http", "url": url, "timeout": 10 } ] } ],
            "SubagentStop": [ { "hooks": [ { "type": "http", "url": url, "timeout": 10 } ] } ],
            "Notification": [ { "hooks": [ { "type": "http", "url": url, "timeout": 10 } ] } ],
            "SessionEnd": [ { "hooks": [ { "type": "http", "url": url, "timeout": 10 } ] } ]
        }
    })
    .to_string()
}

/// A 32-char random hook token (per-session URL secret).
fn mint_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..32).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
}
