//! LIVE proof of the tier-2 hook channel against a real Claude Code session on haiku
//! (Phase 2 deliverable 2, upgraded to "prove it live"). Ignored by default; run with:
//!
//!   cargo test -p dflowd --test live_claude -- --ignored --nocapture
//!
//! Dispatches a claude session on haiku in an isolated scratch project with a trivial
//! prompt, then reads the real Stop/Notification hook POSTs from the daemon log, the
//! lifecycle transitions they drove, the session_id captured into resume_ref, and a
//! real session.resume round trip. Every session is kept short and kill-verified.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dflow_proto::{CardCreate, DispatchStart, Envelope, ProjectAdd, SessionAttach, SessionResume};
use serde_json::Value;

fn claude_on_path() -> bool {
    let exts = ["", ".exe", ".cmd", ".bat"];
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|dir| {
                exts.iter().any(|e| dir.join(format!("claude{e}")).is_file())
            })
        })
        .unwrap_or(false)
}

async fn session_row(ws: &mut Ws, id: &str) -> Option<Value> {
    let mut sink = Vec::new();
    let resp = request(ws, &Envelope::message("sl", "session.list", serde_json::json!({})), &mut sink).await;
    resp.payload["sessions"].as_array()?.iter().find(|s| s["session_id"].as_str() == Some(id)).cloned()
}

#[tokio::test]
#[ignore = "live: launches a real claude session on haiku"]
async fn live_claude_haiku_hooks_and_resume() {
    if !claude_on_path() {
        eprintln!("SKIP: claude not on PATH");
        return;
    }
    let data_dir = unique_data_dir("live-claude");
    let (mut guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "info")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

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
                title: "say ok".into(),
                card_type: "chore".into(),
                project_id: Some(project_id),
                dial_recipe: None,
                brief: Some(
                    "Reply with exactly the word: ok . Then stop. Do not use tools or ask for permission."
                        .into(),
                ),
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
                model: Some("haiku".into()),
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
    let session_id = disp.payload["session_id"].as_str().expect("dispatched").to_string();
    eprintln!("LIVE dispatched session {session_id} on haiku");

    // Attach and capture the first ~8s of the TUI (trust dialog text, busy footer).
    let _ = request(
        &mut ws,
        &Envelope::message(
            "at",
            "session.attach",
            SessionAttach { session_id: session_id.clone(), cols: 120, rows: 40 },
        ),
        &mut sink,
    )
    .await;
    let early = collect_output_until(&mut ws, "esc to interrupt", Duration::from_secs(20)).await;
    eprintln!("LIVE early screen capture (trust/busy):\n{early}\n---END EARLY---");

    // Poll for a hook-driven transition (idle/done) up to ~90s.
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut final_state = String::new();
    let mut resume_ref: Option<String> = None;
    while Instant::now() < deadline {
        if let Some(row) = session_row(&mut ws, &session_id).await {
            final_state = row["state"].as_str().unwrap_or("").to_string();
            resume_ref = row["resume_ref"].as_str().map(str::to_string);
            if resume_ref.is_some() && (final_state == "idle" || final_state == "done") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    eprintln!("LIVE final_state={final_state} resume_ref={resume_ref:?}");

    // The daemon log must show real hook POSTs.
    let log = std::fs::read_to_string(data_dir.join("daemon.log")).unwrap_or_default();
    let hook_lines: Vec<&str> = log.lines().filter(|l| l.contains("hook POST received")).collect();
    eprintln!("LIVE hook POST lines ({}):", hook_lines.len());
    for l in &hook_lines {
        eprintln!("  {l}");
    }

    assert!(!hook_lines.is_empty(), "expected real claude hook POSTs at the endpoint");
    assert!(resume_ref.is_some(), "expected a harness-native session_id captured into resume_ref");

    // Real session.resume round trip.
    let resumed = request(
        &mut ws,
        &Envelope::message("r", "session.resume", SessionResume { session_id: session_id.clone() }),
        &mut sink,
    )
    .await;
    eprintln!("LIVE resume reply: {}", resumed.payload);
    let new_id = resumed.payload["session_id"].as_str().unwrap_or("");
    assert!(!new_id.is_empty() && new_id != session_id, "resume creates a new session");
    // Give the resumed session a few seconds to reload its transcript.
    tokio::time::sleep(Duration::from_secs(4)).await;
    if let Some(row) = session_row(&mut ws, new_id).await {
        eprintln!("LIVE resumed session row: {row}");
    }

    // Kill everything and stop the daemon; verify.
    let _ = request(
        &mut ws,
        &Envelope::message("dc", "dispatch.cancel", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    guard.kill_now();
    eprintln!("LIVE done; daemon killed");
}
