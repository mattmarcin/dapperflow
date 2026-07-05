//! session.resume v1 end-to-end (`architecture.md` / session resume; Phase 2
//! deliverables 6 and 8).
//!
//! Dispatches a claude-family session through a configured launcher whose command is a
//! stub that echoes its argv and then loops, captures a harness-native resume_ref via a
//! hook POST, then resumes the session and proves the relaunch used `--resume <ref>`,
//! created a new row linked via `resumed_from`, and emitted the scrollback-divider
//! event. Isolated DFLOW_DATA_DIR.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::{
    AgentAdd, CardCreate, DispatchStart, Envelope, ProjectAdd, SessionAttach, SessionResume,
};
use serde_json::Value;

/// A stub that echoes its argv (so the resume flag is observable) then loops forever.
fn write_echo_stub(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("resumestub.cmd");
    std::fs::write(
        &path,
        "@echo off\r\necho DFLOW_ARGS: %*\r\n:loop\r\nping -n 3 127.0.0.1 >nul 2>&1\r\ngoto loop\r\n",
    )
    .unwrap();
    path
}

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

fn post_hook(port: u16, token: &str, body: &Value) {
    let payload = serde_json::to_vec(body).unwrap();
    let req = format!(
        "POST /hooks/{token} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect hook endpoint");
    stream.write_all(req.as_bytes()).unwrap();
    stream.write_all(&payload).unwrap();
    let mut resp = String::new();
    let _ = stream.read_to_string(&mut resp);
}

#[tokio::test]
async fn resume_relaunches_with_resume_ref_and_links_lineage() {
    let data_dir = unique_data_dir("resume");
    let stub = write_echo_stub(&data_dir);
    let (mut guard, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // A configured claude-family launcher whose command is the echoing stub.
    request(
        &mut ws,
        &Envelope::message(
            "a",
            "agents.add",
            AgentAdd {
                name: "cstub".into(),
                adapter: "claude".into(),
                command: stub.to_string_lossy().into_owned(),
                extra_args: vec![],
                extra_env: Default::default(),
            },
        ),
        &mut sink,
    )
    .await;

    let repo = scratch_repo(&data_dir);
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
                title: "resume me".into(),
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
                agent: Some("cstub".into()),
                harness: None,
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
    let orig_id = disp.payload["session_id"].as_str().unwrap().to_string();

    // Capture a harness-native resume_ref via a hook POST.
    let (hook_port, hook_token) = read_hook_endpoint(&data_dir);
    post_hook(
        hook_port,
        &hook_token,
        &serde_json::json!({ "hook_event_name": "Stop", "session_id": "claude-sess-777" }),
    );

    // Give the capture a moment, then resume.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let resumed = request(
        &mut ws,
        &Envelope::message(
            "r",
            "session.resume",
            SessionResume { session_id: orig_id.clone() },
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resumed.msg_type, "session.resume", "resume replied, got {:?}", resumed);
    let new_id = resumed.payload["session_id"].as_str().expect("new session id").to_string();
    assert_ne!(new_id, orig_id, "resume creates a NEW session row");
    assert_eq!(resumed.payload["resumed_from"].as_str(), Some(orig_id.as_str()));
    assert_eq!(resumed.payload["resume_ref"].as_str(), Some("claude-sess-777"));

    // Attach to the resumed session and prove the relaunch used --resume <ref>.
    let attach = Envelope::message(
        "at",
        "session.attach",
        SessionAttach { session_id: new_id.clone(), cols: 120, rows: 32 },
    );
    let _ = request(&mut ws, &attach, &mut sink).await;
    let output = collect_output_until(&mut ws, "--resume", Duration::from_secs(8)).await;
    assert!(output.contains("--resume"), "resumed launch missing --resume, got: {output:?}");
    assert!(output.contains("claude-sess-777"), "resumed launch missing the resume ref: {output:?}");

    // The card timeline recorded the session_resumed divider event.
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
    let has_divider = cget.payload["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["kind"].as_str() == Some("session_resumed"));
    assert!(has_divider, "resume should append a session_resumed event for the UI divider");

    let _ = request(
        &mut ws,
        &Envelope::message("dc", "dispatch.cancel", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    guard.kill_now();
}
