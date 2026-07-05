//! `dflowd --status` / `--stop` and graceful-shutdown semantics (Phase 2 daemon
//! lifecycle). Dispatches a long-lived stub session, checks status reports it, stops
//! the daemon gracefully, and asserts the session was marked `interrupted` (resumable)
//! rather than `done` - and that a second stop reports nothing running.

mod common;

use std::process::Command;
use std::time::Duration;

use common::*;
use dflow_proto::{CardCreate, DispatchStart, Envelope, ProjectAdd};

fn write_loop_stub(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("lifestub.cmd");
    std::fs::write(&path, "@echo off\r\n:loop\r\nping -n 3 127.0.0.1 >nul 2>&1\r\ngoto loop\r\n")
        .unwrap();
    path
}

/// Run `dflowd <args> --data-dir <dir>` and return `(exit_code, stdout)`.
fn run_dflowd(data_dir: &std::path::Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_dflowd"))
        .args(args)
        .arg("--data-dir")
        .arg(data_dir)
        .env("DFLOW_LOG", "error")
        .output()
        .expect("run dflowd control");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string())
}

#[tokio::test]
async fn status_stop_and_graceful_interrupt() {
    let data_dir = unique_data_dir("lifecycle");
    let stub = write_loop_stub(&data_dir);
    let launch = serde_json::to_string(&vec![stub.to_string_lossy().into_owned()]).unwrap();
    let (guard, port, token) =
        start_daemon(&data_dir, &[("DFLOW_LAUNCH_CLAUDE", launch.as_str())]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Dispatch a long-lived claude session.
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
                title: "lifecycle".into(),
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
                card_id,
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
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    // --status reports the running daemon and its live session.
    let (code, out) = run_dflowd(&data_dir, &["--status"]);
    assert_eq!(code, 0);
    assert!(out.contains("running"), "status should report running, got: {out}");
    assert!(out.contains("live_sessions=1"), "status should count the live session, got: {out}");

    // --stop shuts it down gracefully (exit 0).
    let (code, out) = run_dflowd(&data_dir, &["--stop"]);
    assert_eq!(code, 0, "stop should succeed, out: {out}");
    assert!(out.contains("stopped"), "stop should report stopped, got: {out}");
    // The daemon subprocess is gone; releasing the guard is a harmless no-op.
    drop(guard);

    // The gracefully-stopped session is marked interrupted (resumable), not done.
    let store = dflow_core::Store::open(&data_dir.join("store.db")).unwrap();
    let row = store.get_session(&session_id).unwrap().expect("session row survives");
    assert_eq!(row.state, "interrupted", "graceful shutdown marks sessions interrupted");
    drop(store);

    // --status now reports not running; a second --stop reports nothing to stop.
    let (_, out) = run_dflowd(&data_dir, &["--status"]);
    assert!(out.contains("not running"), "status after stop should be not running, got: {out}");
    let (code, out) = run_dflowd(&data_dir, &["--stop"]);
    assert_eq!(code, 3, "stopping a dead daemon returns the not-running code, out: {out}");

    // Give Windows a moment to release the stub's file handles before temp cleanup.
    tokio::time::sleep(Duration::from_millis(200)).await;
}
