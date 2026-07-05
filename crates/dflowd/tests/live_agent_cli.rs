//! Live proof (opt-in, `#[ignore]`): a real `claude --model haiku` session driving the
//! `dflow` CLI for real (`agent-cli.md` live-proof deliverable).
//!
//! Run with: `cargo build -p dflow-cli && cargo test -p dflowd --test live_agent_cli -- --ignored --nocapture`
//!
//! Requires the `claude` CLI on PATH. It dispatches a haiku session whose brief tells
//! it to run a handful of dflow commands, then verifies the daemon and the on-disk
//! knowledgebase actually changed. Uses DFLOW_DATA_DIR isolation so it never touches a
//! live user daemon.

mod common;

use std::time::Duration;

use common::*;
use dflow_proto::{decode_frame, Envelope, FrameKind};
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const BRIEF: &str = "\
You are proving the dflow CLI works. Run EXACTLY these shell commands in order, then reply `done` and stop. \
Do not edit files or do anything else.

1. dflow status working \"haiku is driving dflow\"
2. dflow card note \"ran the status verb\"
3. dflow card create --title \"follow-up from haiku\" --type test
4. printf 'Haiku proved the knowledge write path.\\n' | dflow know add --type gotcha --title \"haiku proof\" --stdin
5. dflow status done \"proof complete\"";

#[tokio::test]
#[ignore = "live: requires the claude CLI; run explicitly with --ignored"]
async fn live_haiku_drives_dflow() {
    let data_dir = unique_data_dir("live-agentcli");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    let cadd = request(
        &mut ws,
        &Envelope::message("c", "card.create", serde_json::json!({ "title": "haiku dflow proof", "type": "chore", "project_id": project_id, "brief": BRIEF })),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Register a throwaway launcher that skips permission prompts so haiku can run the
    // brief's shell commands unattended (isolated scratch repo; test-only).
    let _ = request(
        &mut ws,
        &Envelope::message("ag", "agents.add", serde_json::json!({ "name": "haiku-yolo", "adapter": "claude", "command": "claude", "extra_args": ["--dangerously-skip-permissions"] })),
        &mut sink,
    )
    .await;

    let disp = request(
        &mut ws,
        &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "agent": "haiku-yolo", "model": "haiku" })),
        &mut sink,
    )
    .await;
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();
    println!("dispatched session {session_id} for card {card_id}");

    let _ = request(&mut ws, &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 140, "rows": 40 })), &mut sink).await;

    // Poll the daemon + on-disk knowledgebase for the real effects (screen scraping is
    // unreliable: the brief itself echoes the commands). The agent runs the brief's
    // shell commands autonomously; give haiku up to 3 minutes.
    let note_path = repo.join("docs/knowledge/gotchas/haiku-proof.md");
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
    let mut kinds: Vec<String>;
    let mut titles: Vec<String>;
    let mut screen: Vec<u8> = Vec::new();
    loop {
        // Drain PTY output for a slice so we can diagnose what the agent actually ran.
        let slice = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < slice {
            match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
                Ok(Some(Ok(WsMessage::Binary(b)))) => {
                    if let Ok(f) = decode_frame(&b) {
                        if f.kind == FrameKind::Output {
                            screen.extend_from_slice(&f.data);
                        }
                    }
                }
                Ok(Some(Ok(_))) => {}
                _ => break,
            }
        }
        let cget = request(&mut ws, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id })), &mut sink).await;
        kinds = cget.payload["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str().map(String::from)).collect();
        let query = request(&mut ws, &Envelope::message("q", "card.query", serde_json::json!({ "filter": { "project_id": project_id } })), &mut sink).await;
        titles = query.payload["cards"].as_array().unwrap().iter().filter_map(|c| c["title"].as_str().map(String::from)).collect();
        let filed = titles.iter().any(|t| t.contains("follow-up from haiku"));
        if note_path.exists() && filed {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
    }
    // Surface any dflow-related lines from the transcript for diagnosis.
    let text = String::from_utf8_lossy(&screen);
    let relevant: Vec<&str> = text
        .lines()
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("dflow") || l.contains("not found") || l.contains("recorded")
                || l.contains("created card") || l.contains("command") || l.contains("permission")
        })
        .collect();
    println!("---- dflow-relevant transcript lines ----");
    for l in relevant.iter().rev().take(25).rev() {
        println!("{}", l.trim());
    }

    println!("---- live evidence ----");
    println!("knowledge note on disk ({}): {}", note_path.display(), note_path.exists());
    if note_path.exists() {
        println!("note contents:\n{}", std::fs::read_to_string(&note_path).unwrap());
        println!("index.md:\n{}", std::fs::read_to_string(repo.join("docs/knowledge/index.md")).unwrap_or_default());
    }
    println!("card events on the dispatch card: {kinds:?}");
    println!("cards now in project: {titles:?}");

    assert!(note_path.exists(), "haiku did not write the knowledge note");
    assert!(kinds.iter().any(|k| k == "knowledge_updated"), "no knowledge_updated event: {kinds:?}");
    assert!(titles.iter().any(|t| t.contains("follow-up from haiku")), "haiku did not file the follow-up card: {titles:?}");
    println!("LIVE PROOF PASSED: haiku drove dflow status, card create, and know add for real.");
}
