//! LIVE busy-signature capture for claude (Phase 2 live-probe checklist item 1).
//! Ignored by default:
//!   cargo test -p dflowd --test live_busy -- --ignored --nocapture

mod common;

use std::time::Duration;

use common::*;
use dflow_proto::{CardCreate, DispatchStart, Envelope, ProjectAdd, SessionAttach};

#[tokio::test]
#[ignore = "live: launches a real claude session on haiku"]
async fn live_claude_busy_footer() {
    let exts = ["", ".exe", ".cmd"];
    let have = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| exts.iter().any(|e| d.join(format!("claude{e}")).is_file())))
        .unwrap_or(false);
    if !have {
        eprintln!("SKIP: claude not on PATH");
        return;
    }
    let data_dir = unique_data_dir("live-busy");
    let (mut guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "warn")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();
    let repo = scratch_repo(&data_dir);
    let padd = request(&mut ws, &Envelope::message("p", "project.add", ProjectAdd { path: repo.to_string_lossy().into() }), &mut sink).await;
    let pid = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", CardCreate {
        title: "count".into(), card_type: "chore".into(), project_id: Some(pid),
        dial_recipe: None, brief: Some("List the numbers 1 through 30, one per line. Do not ask for permission.".into()),
        priority: None, lane: None, fingerprint: None,
    }), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();
    let disp = request(&mut ws, &Envelope::message("d", "dispatch.start", DispatchStart {
        card_id: card_id.clone(), recipe: None, agent: None, harness: Some("claude".into()),
        model: Some("haiku".into()), effort: None, budget_cards: None, budget_notes: None, audit: false,
                ack_in_place: false,
    }), &mut sink).await;
    let sid = disp.payload["session_id"].as_str().unwrap().to_string();
    let _ = request(&mut ws, &Envelope::message("at", "session.attach", SessionAttach { session_id: sid.clone(), cols: 120, rows: 40 }), &mut sink).await;

    // Collect until the busy footer appears (agent working after trust auto-answer).
    let out = collect_output_until(&mut ws, "interrupt", Duration::from_secs(45)).await;
    // Print the lines around the footer for the spike record.
    for line in out.lines().filter(|l| l.to_lowercase().contains("interrupt")) {
        eprintln!("BUSY_FOOTER: {}", line.trim());
    }
    eprintln!("BUSY_FOOTER_FOUND: {}", out.to_lowercase().contains("esc to interrupt"));

    let _ = request(&mut ws, &Envelope::message("dc", "dispatch.cancel", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    guard.kill_now();
}
