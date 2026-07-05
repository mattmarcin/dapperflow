//! End-to-end persistence proof: the Phase 0 acceptance test.
//!
//! Runs the real `dflowd.exe`, creates a PowerShell session, produces output over
//! one WebSocket connection, drops that connection (as closing the GUI would),
//! reconnects on a fresh connection, and proves the session survived with its
//! scrollback intact on reattach. This is the heart of the product.

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{
    encode_frame, Envelope, FrameKind, SessionAttach, SessionAttached, SessionCreate,
    SessionCreated, SessionListResult,
};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const MARKER: &str = "REATTACH_PROOF_7Q";

#[tokio::test]
async fn session_survives_reconnect_with_scrollback() {
    let data_dir = unique_data_dir("e2e");
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);

    // ---- Connection 1: create a session and produce output. ----
    let mut ws = connect_and_auth(port, &token).await;

    let mut sink = Vec::new();
    let create = Envelope::message(
        "c1",
        "session.create",
        SessionCreate {
            card_id: None,
            worktree_id: None,
            harness: "powershell".into(),
        first_prompt: None,
            agent: None,
            command: None,
            cols: 100,
            rows: 30,
            cwd: None,
            env: Default::default(),
        },
    );
    let resp = request(&mut ws, &create, &mut sink).await;
    assert_eq!(resp.msg_type, "session.create", "create response: {resp:?}");
    let created: SessionCreated = resp.decode_payload().unwrap();
    let session_id = created.session_id.clone();

    // Attach so live output flows, then let the shell start and type a command.
    let attach = Envelope::message(
        "a1",
        "session.attach",
        SessionAttach { session_id: session_id.clone(), cols: 100, rows: 30 },
    );
    let resp = request(&mut ws, &attach, &mut sink).await;
    assert_eq!(resp.msg_type, "session.attach", "attach response: {resp:?}");

    // Give the shell a moment to draw its prompt, then send the marker command.
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let sid = ulid::Ulid::from_string(&session_id).unwrap().to_bytes();
    let input = encode_frame(FrameKind::Input, &sid, format!("echo {MARKER}\r\n").as_bytes());
    ws.send(WsMessage::Binary(input.into())).await.unwrap();

    let seen = collect_output_until(&mut ws, MARKER, Duration::from_secs(15)).await;
    assert!(seen.contains(MARKER), "marker never appeared in live output: {seen:?}");

    // ---- Drop connection 1 entirely, as closing the GUI would. ----
    drop(ws);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ---- Connection 2: the session must still exist and replay its scrollback. ----
    let mut ws2 = connect_and_auth(port, &token).await;

    // It appears in the fleet list on a brand-new connection.
    let mut sink2 = Vec::new();
    let list = Envelope::message("l1", "session.list", serde_json::json!({}));
    let resp = request(&mut ws2, &list, &mut sink2).await;
    let listing: SessionListResult = resp.decode_payload().unwrap();
    assert!(
        listing.sessions.iter().any(|s| s.session_id == session_id && s.alive),
        "session missing from list after reconnect: {listing:?}"
    );

    // Reattach: the response replay must contain the earlier marker output.
    let attach2 = Envelope::message(
        "a2",
        "session.attach",
        SessionAttach { session_id: session_id.clone(), cols: 100, rows: 30 },
    );
    let resp = request(&mut ws2, &attach2, &mut sink2).await;
    let attached: SessionAttached = resp.decode_payload().unwrap();
    let replay = base64::engine::general_purpose::STANDARD.decode(&attached.replay_base64).unwrap();
    let replay_text = String::from_utf8_lossy(&replay);
    assert!(
        replay_text.contains(MARKER),
        "reattach replay did not contain prior scrollback marker; replay was: {replay_text:?}"
    );

    // The styled snapshot should also reflect a non-empty screen.
    assert!(attached.snapshot.rows > 0 && attached.snapshot.cols > 0, "empty snapshot dims");

    // ---- Clean shutdown. ----
    let shutdown = Envelope::message("s1", "daemon.shutdown", serde_json::json!({}));
    let mut junk = Vec::new();
    let _ = request(&mut ws2, &shutdown, &mut junk).await;
}
