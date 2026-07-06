//! Gate reviewer brief delivery e2e (`gate.md` / Adversarial review; `adapters.md` /
//! Dispatch brief delivery). The gate REQUIRES the reviewer harness to differ from the
//! author's, so the default cross-model pairing (claude author -> codex/opencode reviewer)
//! puts the reviewer on a shim harness launched through `cmd.exe /c`. `cmd.exe` truncates a
//! multi-line launch argument at the first newline, so a launch-argument reviewer brief
//! arrived as only its first line - the reviewer never saw the diff, the acceptance criteria,
//! or the `dflow finding add` contract that sit below the fold.
//!
//! These drive the real `gate.run` pipeline against the in-repo stub TUI standing in for a
//! shim reviewer CLI (following `crates/dflowd/tests/brief_delivery.rs`) and prove:
//! 1. a below-the-first-newline token from the reviewer brief reaches the reviewer's input
//!    (typed delivery, not truncated on cmd.exe), and
//! 2. a reviewer whose brief delivery FAILS escalates the gate run honestly rather than
//!    letting a briefless, finding-less reviewer count as a silent pass.
//!
//! The stub TUI (`dflow-stubtui`) is a workspace binary; under `cargo test --workspace` it is
//! built before this test runs. Run in isolation without it built, the delivery test skips.

#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::Envelope;
use serde_json::json;

/// The stub-TUI binary sits next to `dflowd.exe` in the same target dir.
fn stub_tui_path() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_BIN_EXE_dflowd")).parent()?.join("dflow-stubtui.exe");
    p.is_file().then_some(p)
}

/// The HEAD commit sha of a repo (`git rev-parse HEAD`).
fn git_head(repo: &Path) -> String {
    let out =
        std::process::Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Write a project-scope `gate: full` recipe whose `## verify` guidance carries `verify_body`.
/// The verify guidance is spliced into the reviewer brief BELOW its first line (the reviewer
/// preamble is line 1), so a distinctive token placed here lands exactly where `cmd.exe`
/// truncation would drop it.
fn write_gate_recipe(repo: &Path, verify_body: &str) {
    let dir = repo.join(".dapperflow").join("recipes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("gatefull.md"),
        format!(
            "---\nname: gatefull\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: full\n  reviewer_harness: different\nship:\n  target: pr\n---\n\n## verify\n{verify_body}\n"
        ),
    )
    .unwrap();
}

/// A `.cmd` shim that exports `DFLOW_STUB_CAPTURE` (so the stub mirrors its typed input) and
/// launches the stub TUI. This is the exact shape of a real codex/opencode launch (a `*.cmd`
/// next to a `#!/bin/sh` shim): `launchable_command` resolves it through `cmd.exe /c`, the
/// launch form whose multi-line argument cmd.exe truncates. Setting the capture var inside the
/// shim itself sidesteps daemon-env-to-child propagation, which portable-pty does not
/// guarantee on Windows.
fn write_stub_shim(data_dir: &Path, name: &str, stub: &Path, capture: &Path) -> PathBuf {
    let shim = data_dir.join(name);
    std::fs::write(
        &shim,
        format!(
            "@echo off\r\nset \"DFLOW_STUB_CAPTURE={}\"\r\n\"{}\"\r\n",
            capture.display(),
            stub.display()
        ),
    )
    .unwrap();
    shim
}

/// A `.cmd` shim that exits immediately with no composer - used to force a typed-delivery
/// failure (the session dies before it is ever ready to accept the brief).
fn write_exit_shim(data_dir: &Path, name: &str) -> PathBuf {
    let shim = data_dir.join(name);
    std::fs::write(&shim, "@echo off\r\nexit /b 0\r\n").unwrap();
    shim
}

/// `DFLOW_LAUNCH_<HARNESS>` value pointing at a `.cmd` shim (no `{brief}` seam - the brief is
/// delivered typed, not on argv).
fn launch_override(shim: &Path) -> String {
    json!([shim.to_string_lossy()]).to_string()
}

async fn poll_gate_status(ws: &mut Ws, card_id: &str, timeout: Duration) -> serde_json::Value {
    let mut sink = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        let st = request(
            ws,
            &Envelope::message("gs", "gate.status", json!({ "card_id": card_id })),
            &mut sink,
        )
        .await;
        let status = st.payload["run"]["status"].as_str().unwrap_or("");
        if matches!(status, "passed" | "failed" | "escalated") {
            return st.payload;
        }
        assert!(Instant::now() < deadline, "gate never reached a terminal status: {:?}", st.payload);
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

#[tokio::test]
async fn gate_reviewer_brief_reaches_a_shim_reviewer_below_the_fold() {
    let Some(stub) = stub_tui_path() else {
        eprintln!("SKIP: dflow-stubtui.exe not built (run under `cargo test --workspace`)");
        return;
    };

    let data_dir = unique_data_dir("gate-brief");
    let repo = scratch_repo(&data_dir);
    // A distinctive token that requires reading the reviewer brief BELOW its first newline.
    // It appears nowhere in the reviewer preamble (line 1), so its presence in the reviewer's
    // input proves the whole multi-line brief arrived, not just the truncated first line.
    let token = "gatebelowfold5555";
    write_gate_recipe(&repo, &format!("Run the full gate. Below-the-fold acceptance token: {token}."));

    let capture = data_dir.join("reviewer-input.txt");
    let shim = write_stub_shim(&data_dir, "opencode-gate-shim.cmd", &stub, &capture);
    // opencode's manifest declares typed delivery; point the opencode harness at the shim.
    let opencode_launch = launch_override(&shim);

    let (_daemon, port, token_auth) = start_daemon(
        &data_dir,
        &[
            ("DFLOW_LAUNCH_OPENCODE", opencode_launch.as_str()),
            // Keep the review step bounded: the stub never self-exits, so it is killed at the
            // session timeout after the brief is already delivered and captured.
            ("DFLOW_GATE_SESSION_TIMEOUT_MS", "8000"),
        ],
    );
    let mut ws = connect_and_auth(port, &token_auth).await;
    let mut sink = Vec::new();

    let padd = request(
        &mut ws,
        &Envelope::message("p", "project.add", json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(
        &mut ws,
        &Envelope::message(
            "c",
            "card.create",
            json!({ "title": "gate brief card", "type": "feature", "project_id": project_id, "dial_recipe": "gatefull" }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Seed a commit to gate (main HEAD). Author is claude (native exe); the reviewer differs
    // and resolves to opencode (a shim harness) - the exact default cross-model pairing.
    let head = git_head(&repo);
    let gr = request(
        &mut ws,
        &Envelope::message(
            "g",
            "gate.run",
            json!({ "card_id": card_id, "head_sha": head, "author_harness": "claude", "reviewer_harness": "opencode" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(gr.msg_type, "gate.run", "gate.run failed: {gr:?}");
    assert_eq!(gr.payload["strictness"], "full", "the recipe is gate: full: {gr:?}");

    // Typed injection is readiness-gated and runs on the gate pipeline thread; poll the
    // reviewer's captured input for the below-the-fold token.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut captured = String::new();
    while Instant::now() < deadline {
        captured = std::fs::read_to_string(&capture).unwrap_or_default();
        if captured.contains(token) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // The reviewer preamble (line 1) AND the below-the-fold acceptance token both arrived. A
    // truncated launch-argument brief would have delivered only the preamble and dropped
    // everything past the first newline - including the token, the diff, and the finding
    // contract - so the reviewer could not review at all.
    assert!(
        captured.contains("adversarial reviewer"),
        "the reviewer preamble never reached the reviewer: {captured:?}"
    );
    assert!(
        captured.contains(token),
        "the below-the-fold acceptance token was truncated - the gate reviewer brief is NOT \
         delivered in full on a shim harness: {captured:?}"
    );
}

#[tokio::test]
async fn gate_reviewer_brief_delivery_failure_escalates_never_passes() {
    let data_dir = unique_data_dir("gate-brief-fail");
    let repo = scratch_repo(&data_dir);
    write_gate_recipe(&repo, "Run the full gate.");

    // A typed-delivery harness (opencode) whose launch dies immediately: the composer never
    // becomes ready, so the readiness-gated verified-submit cannot deliver the brief.
    let shim = write_exit_shim(&data_dir, "opencode-dead-shim.cmd");
    let opencode_launch = launch_override(&shim);

    let (_daemon, port, token_auth) = start_daemon(
        &data_dir,
        &[
            ("DFLOW_LAUNCH_OPENCODE", opencode_launch.as_str()),
            ("DFLOW_GATE_SESSION_TIMEOUT_MS", "8000"),
        ],
    );
    let mut ws = connect_and_auth(port, &token_auth).await;
    let mut sink = Vec::new();

    let padd = request(
        &mut ws,
        &Envelope::message("p", "project.add", json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(
        &mut ws,
        &Envelope::message(
            "c",
            "card.create",
            json!({ "title": "gate brief fail card", "type": "feature", "project_id": project_id, "dial_recipe": "gatefull" }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    let head = git_head(&repo);
    request(
        &mut ws,
        &Envelope::message(
            "g",
            "gate.run",
            json!({ "card_id": card_id, "head_sha": head, "author_harness": "claude", "reviewer_harness": "opencode" }),
        ),
        &mut sink,
    )
    .await;

    let status = poll_gate_status(&mut ws, &card_id, Duration::from_secs(40)).await;

    // A reviewer that never received its brief must NOT pass: the run fails honestly. (A
    // briefless reviewer files no findings, which under the old argv path would have looked
    // like a clean pass.)
    assert_eq!(
        status["run"]["status"], "failed",
        "a failed reviewer brief delivery must fail the run, never pass: {status:?}"
    );
    assert!(
        status["findings"].as_array().map(|f| f.is_empty()).unwrap_or(true),
        "no findings when the reviewer never got its brief: {status:?}"
    );

    // The timeline records the honest reason on both the review step and the gate_failed event.
    let cget = request(
        &mut ws,
        &Envelope::message("cg", "card.get", json!({ "card_id": card_id, "events_limit": 200 })),
        &mut sink,
    )
    .await;
    let events = cget.payload["events"].as_array().unwrap();
    let review = events
        .iter()
        .find(|e| e["kind"] == "gate_step" && e["payload"]["step"] == "review")
        .expect("a review gate_step is recorded");
    assert_eq!(review["payload"]["status"], "failed", "the review step is failed: {review:?}");
    assert!(
        review["payload"]["evidence"]["reason"]
            .as_str()
            .unwrap_or("")
            .contains("brief delivery failed"),
        "the review step names the honest brief-delivery failure: {review:?}"
    );
    let failed = events
        .iter()
        .find(|e| e["kind"] == "gate_failed")
        .expect("a gate_failed event is recorded");
    assert!(
        failed["payload"]["reason"].as_str().unwrap_or("").contains("brief delivery failed"),
        "the gate_failed reason names the brief-delivery failure: {failed:?}"
    );

    // A gate_passed event must NOT exist - the empty review never counted as a pass.
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap_or("")).collect();
    assert!(!kinds.contains(&"gate_passed"), "the run must never emit gate_passed: {kinds:?}");
}
