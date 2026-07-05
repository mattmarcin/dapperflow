//! Daemon-restart reconciliation end to end (deliverable 7, `architecture.md` /
//! daemon restarts): kill the daemon mid-session, restart it on the same data dir,
//! and the session row is marked `interrupted` (never deleted) with a
//! `state_changed` event, and no orphan stub process survives the kill.

mod common;

use std::time::Duration;

use common::*;
use dflow_proto::{CardCreated, CardGetResult, DispatchStarted, Envelope, ProjectAdded, SessionListResult};

/// A uniquely named stub so we can look for orphans of exactly this test.
const STUB_LAUNCH: &str = r#"["cmd.exe","/d","/k","title DFLOW_RECONCILE_STUB && echo up"]"#;

#[tokio::test]
async fn daemon_restart_marks_sessions_interrupted() {
    let data_dir = unique_data_dir("reconcile");
    let repo = scratch_repo(&data_dir);

    // ---- Run 1: dispatch a stub session, then kill the daemon (a crash). ----
    let (mut daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let card_id;
    let session_id;
    {
        let mut ws = connect_and_auth(port, &token).await;
        let mut sink = Vec::new();

        let resp = request(
            &mut ws,
            &Envelope::message("p1", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
            &mut sink,
        )
        .await;
        let added: ProjectAdded = resp.decode_payload().unwrap();

        let resp = request(
            &mut ws,
            &Envelope::message(
                "c1",
                "card.create",
                serde_json::json!({ "title": "Survive a crash", "type": "chore", "project_id": added.project_id }),
            ),
            &mut sink,
        )
        .await;
        let created: CardCreated = resp.decode_payload().unwrap();
        card_id = created.card_id.clone();

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
        session_id = started.session_id;

        // The session is live in run 1.
        let resp = request(
            &mut ws,
            &Envelope::message("l1", "session.list", serde_json::json!({})),
            &mut sink,
        )
        .await;
        let listing: SessionListResult = resp.decode_payload().unwrap();
        assert!(listing.sessions.iter().any(|s| s.session_id == session_id && s.alive));
    }

    // Kill the daemon without any graceful shutdown, as a crash would.
    daemon.kill_now();
    tokio::time::sleep(Duration::from_millis(700)).await;

    // The stub must have died with the daemon (job object kill-on-close): no
    // orphan agent burning tokens invisibly (`architecture.md`). The marker is
    // concatenated at query time so the query never matches its own command line.
    let orphans = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "@(Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like ('*DFLOW_RECONCILE' + '_STUB*') }).Count",
        ])
        .output()
        .expect("run orphan query");
    let count = String::from_utf8_lossy(&orphans.stdout).trim().to_string();
    assert_eq!(count, "0", "stub process survived the daemon kill (found {count})");

    // ---- Run 2: same data dir; startup reconciliation marks it interrupted. ----
    // The crash left a stale runtime file; remove it so the poll below cannot read
    // run 1's dead port. (The real desktop shell health-checks before trusting it.)
    let _ = std::fs::remove_file(data_dir.join("runtime.json"));
    let (_daemon2, port2, token2) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port2, &token2).await;
    let mut sink = Vec::new();

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
        .find(|s| s.session_id == session_id)
        .expect("session row must survive the restart, never deleted");
    assert_eq!(row.state, "interrupted", "row: {row:?}");
    assert!(!row.alive);
    assert_eq!(row.card_id.as_deref(), Some(card_id.as_str()));

    // The reconciliation appended a state_changed event with the restart reason.
    let resp = request(
        &mut ws,
        &Envelope::message("g1", "card.get", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    let got: CardGetResult = resp.decode_payload().unwrap();
    let reconciled = got.events.iter().any(|e| {
        e.kind == "state_changed"
            && e.payload["to"] == "interrupted"
            && e.payload["reason"] == "daemon_restart"
    });
    assert!(reconciled, "missing reconcile event; events: {:?}", got.events);

    let _ = request(
        &mut ws,
        &Envelope::message("s1", "daemon.shutdown", serde_json::json!({})),
        &mut sink,
    )
    .await;
}
