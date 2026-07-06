//! End-to-end dispatch brief delivery through the real `dispatch.start` path
//! (`adapters.md` / Dispatch brief delivery): a shim harness (launched through `cmd.exe
//! /c`) must receive its FULL multi-line composed brief - not just the card title.
//!
//! The bug: dispatch passed the composed brief as the `{prompt}` launch argument, and
//! `cmd.exe` truncates a multi-line argument at the first newline, so a codex/opencode/pi
//! agent saw only the card title. The fix delivers a shim harness's brief by typed
//! injection after launch. This drives the whole daemon pipeline with the in-repo stub TUI
//! standing in for a real agent CLI, and asserts a below-the-fold token from the composed
//! brief reached the agent's input.
//!
//! The stub TUI (`dflow-stubtui`) is a workspace binary; under `cargo test --workspace` it
//! is built before this test runs. When run in isolation without it built, the test skips.

#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::{CardCreated, Envelope, ProjectAdded};
use serde_json::json;

/// The stub-TUI binary sits next to `dflowd.exe` in the same target dir.
fn stub_tui_path() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_BIN_EXE_dflowd")).parent()?.join("dflow-stubtui.exe");
    p.is_file().then_some(p)
}

#[tokio::test]
async fn dispatch_delivers_full_brief_typed_on_a_shim_harness() {
    let Some(stub) = stub_tui_path() else {
        eprintln!("SKIP: dflow-stubtui.exe not built (run under `cargo test --workspace`)");
        return;
    };

    let data_dir = unique_data_dir("brief-delivery");
    let repo = scratch_repo(&data_dir);

    // A `.cmd` shim that launches the stub TUI - the exact shape of a real opencode/codex
    // launch (a `*.cmd` next to a `#!/bin/sh` shim). Dispatch resolves it through
    // `cmd.exe /c`, the launch form whose multi-line argument cmd.exe truncates.
    let shim = data_dir.join("opencode-shim.cmd");
    std::fs::write(&shim, format!("@echo off\r\n\"{}\"\r\n", stub.display())).unwrap();
    let capture = data_dir.join("agent-input.txt");

    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Register the repo.
    let resp = request(
        &mut ws,
        &Envelope::message("p1", "project.add", json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let added: ProjectAdded = resp.decode_payload().unwrap();

    // A card whose brief carries a below-the-fold acceptance token: "quokka" lives well
    // past the first newline of the composed brief (title is line 1), exactly where cmd.exe
    // truncation used to cut it off.
    let brief = "Wire up the status endpoint.\n\n\
                 ## Acceptance criteria\n\
                 - reply with the word quokka\n\
                 - then stop";
    let resp = request(
        &mut ws,
        &Envelope::message(
            "c1",
            "card.create",
            json!({
                "title": "Status endpoint card",
                "type": "feature",
                "project_id": added.project_id,
                "brief": brief,
            }),
        ),
        &mut sink,
    )
    .await;
    let created: CardCreated = resp.decode_payload().unwrap();
    let card_id = created.card_id.clone();

    // An opencode launcher (adapter declares typed delivery) pointed at the shim, with the
    // stub's capture file injected into the spawn env so we can read back what the agent
    // received.
    let resp = request(
        &mut ws,
        &Envelope::message(
            "ag",
            "agents.add",
            json!({
                "name": "opencode-stub",
                "adapter": "opencode",
                "command": shim.to_string_lossy(),
                "extra_env": { "DFLOW_STUB_CAPTURE": capture.to_string_lossy() },
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "agents.add", "agents.add failed: {resp:?}");

    // Dispatch through the real pipeline.
    let resp = request(
        &mut ws,
        &Envelope::message(
            "d1",
            "dispatch.start",
            json!({ "card_id": card_id, "agent": "opencode-stub" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "dispatch.start", "dispatch failed: {resp:?}");
    let started: dflow_proto::DispatchStarted = resp.decode_payload().unwrap();
    assert_eq!(started.harness, "opencode");

    // Typed injection is readiness-gated and runs on a background thread; poll the capture
    // for the below-the-fold token.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut captured = String::new();
    while Instant::now() < deadline {
        captured = std::fs::read_to_string(&capture).unwrap_or_default();
        if captured.contains("quokka") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The composed brief reached the agent past its first newline: the card title AND the
    // below-the-fold acceptance token both arrived. A truncated launch-argument brief would
    // have delivered only the title.
    assert!(
        captured.contains("Status endpoint card"),
        "the card title never reached the agent: {captured:?}"
    );
    assert!(
        captured.contains("quokka"),
        "the below-the-fold acceptance token was truncated - dispatch is NOT delivering the \
         full brief on a shim harness: {captured:?}"
    );

    // Clean up.
    let _ = request(
        &mut ws,
        &Envelope::message("x1", "dispatch.cancel", json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    let _ = request(
        &mut ws,
        &Envelope::message("s1", "daemon.shutdown", json!({})),
        &mut sink,
    )
    .await;
}
