//! LIVE daemon-level proof of the reattach snapshot repaint against a real full-screen
//! TUI (Phase 2 reattach fix). Ignored by default:
//!   cargo test -p dflowd --test live_reattach -- --ignored --nocapture
//!
//! Dispatches a real claude session (which runs a full-screen alt-screen TUI), then
//! reattaches on a fresh WS connection and asserts the replay payload the daemon serves
//! is a mode-restoring snapshot repaint (alt-screen enable + a real repaint), not raw
//! ring bytes. The end-to-end xterm wheel-scroll/arrow-key check is a desktop-app (human)
//! verification, out of the daemon's scope.

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{CardCreate, DispatchStart, Envelope, ProjectAdd, SessionAttach};

#[tokio::test]
#[ignore = "live: launches a real claude session on haiku"]
async fn live_reattach_serves_mode_aware_repaint() {
    let exts = ["", ".exe", ".cmd"];
    let have = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| exts.iter().any(|e| d.join(format!("claude{e}")).is_file())))
        .unwrap_or(false);
    if !have {
        eprintln!("SKIP: claude not on PATH");
        return;
    }
    let data_dir = unique_data_dir("live-reattach");
    let (mut guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "warn")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();
    let repo = scratch_repo(&data_dir);
    let padd = request(&mut ws, &Envelope::message("p", "project.add", ProjectAdd { path: repo.to_string_lossy().into() }), &mut sink).await;
    let pid = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", CardCreate {
        title: "reattach".into(), card_type: "chore".into(), project_id: Some(pid),
        dial_recipe: None, brief: Some("Reply ok and stop.".into()), priority: None, lane: None,
        fingerprint: None,
    }), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();
    let disp = request(&mut ws, &Envelope::message("d", "dispatch.start", DispatchStart {
        card_id: card_id.clone(), recipe: None, agent: None, harness: Some("claude".into()),
        model: Some("haiku".into()), effort: None, budget_cards: None, budget_notes: None, audit: false,
                ack_in_place: false,
    }), &mut sink).await;
    let sid = disp.payload["session_id"].as_str().unwrap().to_string();

    // Let the TUI paint (past the trust auto-answer, into the alt-screen composer).
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Reattach on a fresh connection: the replay is the daemon's snapshot repaint.
    let mut ws2 = connect_and_auth(port, &token).await;
    let attached = request(&mut ws2, &Envelope::message("at", "session.attach", SessionAttach { session_id: sid.clone(), cols: 120, rows: 40 }), &mut sink).await;
    let replay_b64 = attached.payload["replay_base64"].as_str().unwrap_or("");
    let replay = base64::engine::general_purpose::STANDARD.decode(replay_b64).unwrap_or_default();
    let replay_str = String::from_utf8_lossy(&replay);
    let alt = replay_str.contains("\u{1b}[?1049h");
    let has_repaint = replay_str.contains("\u{1b}[2J") || replay_str.contains("\u{1b}[H");
    eprintln!("REATTACH replay_bytes={} alt_screen_enable={} has_repaint={}", replay.len(), alt, has_repaint);
    // A full-screen TUI's replay must carry the alt-screen enable and a repaint.
    assert!(has_repaint, "replay must be a snapshot repaint");
    assert!(alt, "a full-screen TUI replay must restore the alt-screen mode");

    let _ = request(&mut ws, &Envelope::message("dc", "dispatch.cancel", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    guard.kill_now();
}
