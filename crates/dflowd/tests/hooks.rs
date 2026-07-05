//! Tier-2 hook endpoint end-to-end (`adapters.md` / tier 2; Phase 2 deliverable 8).
//!
//! Dispatches a claude-family session backed by a long-lived batch stub (so the launch
//! stays alive without a real CLI or tokens), reads the materialized `--settings` file
//! to recover the per-session hook token, then POSTs fake Claude Code hook events to
//! the loopback endpoint and asserts the lifecycle transitions, resume_ref capture, and
//! Needs You events they drive. Isolated `DFLOW_DATA_DIR` as always.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::{CardCreate, DispatchStart, Envelope, ProjectAdd};
use serde_json::Value;

/// A batch stub that ignores its args and loops forever, so the dispatched "claude"
/// session stays alive (the injected `--settings <path>` args are harmless to it).
fn write_loop_stub(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("hookstub.cmd");
    std::fs::write(&path, "@echo off\r\n:loop\r\nping -n 3 127.0.0.1 >nul 2>&1\r\ngoto loop\r\n")
        .unwrap();
    path
}

/// POST a JSON body to the loopback hook endpoint and return the HTTP status line.
fn post_hook(port: u16, token: &str, body: &Value) -> String {
    let payload = serde_json::to_vec(body).unwrap();
    let req = format!(
        "POST /hooks/{token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect hook endpoint");
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(&payload).unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    resp.lines().next().unwrap_or("").to_string()
}

/// Read the single materialized hook settings file and pull `(port, token)` out of the
/// wired URL `http://127.0.0.1:<port>/hooks/<token>`.
fn read_hook_endpoint(data_dir: &std::path::Path) -> (u16, String) {
    let hooks_dir = data_dir.join("hooks");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(entries) = std::fs::read_dir(&hooks_dir) {
            for entry in entries.flatten() {
                let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
                if let Ok(json) = serde_json::from_str::<Value>(&text) {
                    let url = json["hooks"]["Stop"][0]["hooks"][0]["url"].as_str().unwrap_or("");
                    if let Some(rest) = url.strip_prefix("http://127.0.0.1:") {
                        if let Some((port, token)) = rest.split_once("/hooks/") {
                            return (port.parse().unwrap(), token.to_string());
                        }
                    }
                }
            }
        }
        assert!(Instant::now() < deadline, "hook settings file never appeared");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Find one session's `(state, resume_ref)` from session.list.
async fn session_state(ws: &mut Ws, session_id: &str) -> (String, Option<String>) {
    let env = Envelope::message("sl", "session.list", serde_json::json!({}));
    let mut sink = Vec::new();
    let resp = request(ws, &env, &mut sink).await;
    let sessions = resp.payload["sessions"].as_array().cloned().unwrap_or_default();
    for s in sessions {
        if s["session_id"].as_str() == Some(session_id) {
            return (
                s["state"].as_str().unwrap_or("").to_string(),
                s["resume_ref"].as_str().map(str::to_string),
            );
        }
    }
    (String::new(), None)
}

async fn wait_for_state(ws: &mut Ws, session_id: &str, want: &str) -> (String, Option<String>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (state, resume) = session_state(ws, session_id).await;
        if state == want || Instant::now() >= deadline {
            return (state, resume);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

#[tokio::test]
async fn hook_events_drive_lifecycle_and_capture_resume_ref() {
    let data_dir = unique_data_dir("hooks");
    let stub = write_loop_stub(&data_dir);
    let launch = serde_json::to_string(&vec![stub.to_string_lossy().into_owned()]).unwrap();

    let (mut guard, port, token) =
        start_daemon(&data_dir, &[("DFLOW_LAUNCH_CLAUDE", launch.as_str())]);
    let mut ws = connect_and_auth(port, &token).await;

    // Project + card, then dispatch a claude session (the stub keeps it alive).
    let repo = scratch_repo(&data_dir);
    let mut sink = Vec::new();
    let padd = request(
        &mut ws,
        &Envelope::message("p", "project.add", ProjectAdd { path: repo.to_string_lossy().into() }),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    let cadd = request(
        &mut ws,
        &Envelope::message(
            "c",
            "card.create",
            CardCreate {
                title: "hook lifecycle".into(),
                card_type: "chore".into(),
                project_id: Some(project_id),
                dial_recipe: None,
                brief: None,
                priority: None,
                lane: None,
                fingerprint: None,
            },
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    let disp = request(
        &mut ws,
        &Envelope::message(
            "d",
            "dispatch.start",
            DispatchStart {
                card_id: card_id.clone(),
                recipe: None,
                agent: None,
                harness: Some("claude".into()),
                model: None,
                effort: None,
                budget_cards: None,
                budget_notes: None,
                audit: false,
                ack_in_place: false,
            },
        ),
        &mut sink,
    )
    .await;
    let session_id = disp.payload["session_id"].as_str().expect("dispatched session").to_string();

    // Recover the per-session hook endpoint from the materialized --settings file.
    let (hook_port, hook_token) = read_hook_endpoint(&data_dir);
    assert_eq!(hook_port, port, "hook URL targets the daemon port");

    // 1. Notification permission_prompt -> needs_input (trust_dialog), resume_ref captured.
    let status = post_hook(
        hook_port,
        &hook_token,
        &serde_json::json!({
            "hook_event_name": "Notification",
            "notification_type": "permission_prompt",
            "session_id": "claude-native-abc",
            "cwd": "C:/repo"
        }),
    );
    assert!(status.contains("200"), "hook POST should be accepted, got {status}");
    let (state, resume) = wait_for_state(&mut ws, &session_id, "needs_input").await;
    assert_eq!(state, "needs_input", "permission_prompt drives needs_input");
    assert_eq!(resume.as_deref(), Some("claude-native-abc"), "resume_ref captured from hook");

    // 2. Stop -> idle (turn ended), clearing needs_input.
    let status = post_hook(
        hook_port,
        &hook_token,
        &serde_json::json!({ "hook_event_name": "Stop", "session_id": "claude-native-abc" }),
    );
    assert!(status.contains("200"));
    let (state, _) = wait_for_state(&mut ws, &session_id, "idle").await;
    assert_eq!(state, "idle", "Stop drives idle");

    // 3. A newer harness-native id (resume reassigns ids) is re-captured latest-wins.
    post_hook(
        hook_port,
        &hook_token,
        &serde_json::json!({
            "hook_event_name": "Notification",
            "notification_type": "agent_needs_input",
            "session_id": "claude-native-def"
        }),
    );
    let (state, resume) = wait_for_state(&mut ws, &session_id, "needs_input").await;
    assert_eq!(state, "needs_input", "agent_needs_input drives needs_input");
    assert_eq!(resume.as_deref(), Some("claude-native-def"), "latest-wins resume_ref");

    // The card timeline recorded the needs_input + needs_you_raised events.
    let cget = request(
        &mut ws,
        &Envelope::message(
            "cg",
            "card.get",
            serde_json::json!({ "card_id": card_id, "events_limit": 100 }),
        ),
        &mut sink,
    )
    .await;
    let kinds: Vec<String> = cget.payload["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str().map(str::to_string))
        .collect();
    assert!(kinds.iter().any(|k| k == "needs_input"), "needs_input event on the timeline");
    assert!(kinds.iter().any(|k| k == "needs_you_raised"), "needs_you_raised event on the timeline");

    // Clean up the live session and the daemon.
    let _ = request(
        &mut ws,
        &Envelope::message("dc", "dispatch.cancel", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    guard.kill_now();
}
