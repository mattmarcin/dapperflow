//! Dispatch end to end against a stub command (deliverable 10): project.add ->
//! card.create -> dispatch.start launches the stub *in the leased worktree*,
//! persists the session row, and appends the dispatched/worktree_leased/
//! session_started events; event.subscribe streams them; dispatch.cancel kills the
//! session and returns the worktree.
//!
//! The stub is `cmd.exe /k` (NOT a real agent CLI), injected through the
//! `DFLOW_LAUNCH_STUB` override: it echoes the brief and its cwd, then stays alive
//! at the prompt, which is exactly the shape of a long-running harness.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use dflow_proto::{
    CardCreated, CardGetResult, DispatchCancelled, DispatchStarted, Envelope, ProjectAdded,
    SessionListResult,
};

/// The stub launch line: print the cwd first, then echo the brief, and stay alive.
/// `cd` runs before the echo so the cwd is captured even though the composed brief is
/// now multi-line (M2 brief upgrade appends acceptance + digest + the dflow usage
/// contract): a real harness takes the brief as one positional arg, but a cmd.exe stack
/// echoing `{brief}` inline would otherwise let an embedded newline swallow `&& cd`.
const STUB_LAUNCH: &str = r#"["cmd.exe","/d","/k","cd && echo BRIEF[{brief}]"]"#;

#[tokio::test]
async fn dispatch_runs_stub_in_leased_worktree() {
    let data_dir = unique_data_dir("dispatch");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // ---- project.add validates and registers the repo root. ----
    let resp = request(
        &mut ws,
        &Envelope::message("p1", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "project.add", "project.add failed: {resp:?}");
    let added: ProjectAdded = resp.decode_payload().unwrap();
    assert_eq!(added.project.default_branch, "main");

    // Re-adding the same root is idempotent.
    let resp = request(
        &mut ws,
        &Envelope::message("p2", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let again: ProjectAdded = resp.decode_payload().unwrap();
    assert_eq!(again.project_id, added.project_id);

    // A non-repo directory is rejected with a structured error.
    let not_repo = data_dir.join("not-a-repo");
    std::fs::create_dir_all(&not_repo).unwrap();
    let resp = request(
        &mut ws,
        &Envelope::message("p3", "project.add", serde_json::json!({ "path": not_repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "error", "expected error, got {resp:?}");
    assert_eq!(resp.payload["code"], "bad_request");

    // ---- card.create + card.move. ----
    // The brief stays single-line so the cmd.exe stub can echo it faithfully.
    let resp = request(
        &mut ws,
        &Envelope::message(
            "c1",
            "card.create",
            serde_json::json!({
                "title": "Stub the login flow",
                "type": "feature",
                "project_id": added.project_id,
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "card.create", "card.create failed: {resp:?}");
    let created: CardCreated = resp.decode_payload().unwrap();
    let card_id = created.card_id.clone();

    let resp = request(
        &mut ws,
        &Envelope::message("c2", "card.move", serde_json::json!({ "card_id": card_id, "column": "performing" })),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "card.move");

    // ---- dispatch.start with the stub harness. ----
    let resp = request(
        &mut ws,
        &Envelope::message(
            "d1",
            "dispatch.start",
            serde_json::json!({ "card_id": card_id, "harness": "stub" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "dispatch.start", "dispatch failed: {resp:?}");
    let started: DispatchStarted = resp.decode_payload().unwrap();
    assert_eq!(started.harness, "stub");
    let worktree_path = started.worktree_path.clone();
    assert!(Path::new(&worktree_path).is_dir(), "worktree dir missing: {worktree_path}");
    assert!(
        Path::new(&worktree_path).starts_with(data_dir.join("worktrees")),
        "worktree must live under the data dir pool: {worktree_path}"
    );
    assert!(Path::new(&worktree_path).join(".git").exists(), "not a git worktree");

    // ---- event.subscribe with no cursor replays the whole durable log. ----
    // (Subscribed after dispatch so the replay path, not just the live tail, is
    // what delivers the lifecycle events.)
    let resp = request(
        &mut ws,
        &Envelope::message("e1", "event.subscribe", serde_json::json!({})),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "event.subscribe");

    let mut kinds: Vec<String> = Vec::new();
    let got_started = collect_envelope_until(&mut ws, Duration::from_secs(10), |env| {
        if env.msg_type == "event.card_event" {
            if let Some(kind) = env.payload["event"]["kind"].as_str() {
                kinds.push(kind.to_string());
                return kind == "session_started";
            }
        }
        false
    })
    .await;
    assert!(got_started.is_some(), "never saw session_started; kinds so far: {kinds:?}");
    for expected in ["created", "moved", "dispatched", "worktree_leased"] {
        assert!(kinds.iter().any(|k| k == expected), "missing event '{expected}': {kinds:?}");
    }

    // ---- Attach and prove the stub runs in the worktree with the brief. ----
    let resp = request(
        &mut ws,
        &Envelope::message(
            "a1",
            "session.attach",
            serde_json::json!({ "session_id": started.session_id, "cols": 120, "rows": 32 }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "session.attach", "attach failed: {resp:?}");

    // The replay plus live output must show the echoed brief and the cwd.
    let mut seen = String::from_utf8_lossy(&sink).to_string();
    if !seen.contains(&worktree_path) {
        seen.push_str(&collect_output_until(&mut ws, &worktree_path, Duration::from_secs(15)).await);
    }
    assert!(
        seen.contains("BRIEF[Stub the login flow"),
        "stub never echoed the brief; output: {seen:?}"
    );
    assert!(
        seen.contains(&worktree_path),
        "stub cwd is not the worktree; output: {seen:?}"
    );

    // ---- session.list is enriched with card and state. ----
    let resp = request(
        &mut ws,
        &Envelope::message("l1", "session.list", serde_json::json!({})),
        &mut sink,
    )
    .await;
    let listing: SessionListResult = resp.decode_payload().unwrap();
    let row = listing
        .sessions
        .iter()
        .find(|s| s.session_id == started.session_id)
        .expect("dispatched session missing from list");
    assert_eq!(row.card_id.as_deref(), Some(card_id.as_str()));
    assert_eq!(row.harness, "stub");
    assert!(row.alive, "stub session should be alive");
    assert_eq!(row.state, "working");
    assert!(row.first_prompt.as_deref().unwrap_or("").contains("Stub the login flow"));

    // ---- session.rename persists a title. ----
    let resp = request(
        &mut ws,
        &Envelope::message(
            "r1",
            "session.rename",
            serde_json::json!({ "session_id": started.session_id, "title": "My stub" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "session.rename", "rename failed: {resp:?}");

    // ---- fleet.status returns the session table + open Needs You items. ----
    let resp = request(
        &mut ws,
        &Envelope::message("f1", "fleet.status", serde_json::json!({})),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "fleet.status", "fleet.status failed: {resp:?}");
    let fleet: dflow_proto::FleetStatusResult = resp.decode_payload().unwrap();
    assert!(
        fleet.sessions.iter().any(|s| s.session_id == started.session_id),
        "fleet.status missing the dispatched session"
    );
    assert!(fleet.needs_you.iter().all(|n| n.resolved_at.is_none()));

    // ---- card.get returns card + sessions + events. ----
    let resp = request(
        &mut ws,
        &Envelope::message("g1", "card.get", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    let got: CardGetResult = resp.decode_payload().unwrap();
    assert_eq!(got.card.id, card_id);
    assert_eq!(got.card.lane, "performing");
    assert_eq!(got.sessions.len(), 1);
    assert_eq!(got.sessions[0].title.as_deref(), Some("My stub"));
    let event_kinds: Vec<&str> = got.events.iter().map(|e| e.kind.as_str()).collect();
    for expected in ["created", "moved", "dispatched", "worktree_leased", "session_started"] {
        assert!(event_kinds.contains(&expected), "card.get missing '{expected}': {event_kinds:?}");
    }

    // ---- dispatch.cancel kills the session and returns the worktree clean. ----
    let resp = request(
        &mut ws,
        &Envelope::message("x1", "dispatch.cancel", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "dispatch.cancel", "cancel failed: {resp:?}");
    let cancelled: DispatchCancelled = resp.decode_payload().unwrap();
    assert_eq!(cancelled.cancelled, 1);
    assert_eq!(cancelled.worktree_state.as_deref(), Some("available"));

    let resp = request(
        &mut ws,
        &Envelope::message("l2", "session.list", serde_json::json!({})),
        &mut sink,
    )
    .await;
    let listing: SessionListResult = resp.decode_payload().unwrap();
    let row = listing
        .sessions
        .iter()
        .find(|s| s.session_id == started.session_id)
        .expect("session should remain listed after cancel");
    assert!(!row.alive, "cancelled session must not be alive");
    assert_eq!(row.state, "done");

    // ---- Clean shutdown. ----
    let _ = request(
        &mut ws,
        &Envelope::message("s1", "daemon.shutdown", serde_json::json!({})),
        &mut sink,
    )
    .await;
}
