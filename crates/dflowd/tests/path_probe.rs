//! Diagnostic: confirm dispatch prepends the `dflow` binary dir onto the session PATH.

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::Envelope;

const STUB: &str = r#"["cmd.exe","/d","/k","echo PPATH=%Path%"]"#;

#[tokio::test]
async fn dispatch_prepends_dflow_dir_to_path() {
    let data_dir = unique_data_dir("pathprobe");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB), ("DFLOW_LOG", "info")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let pid = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", serde_json::json!({ "title": "p", "type": "chore", "project_id": pid })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();
    let disp = request(&mut ws, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(&mut ws, &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 200, "rows": 32 })), &mut sink).await;
    let replay = base64::engine::general_purpose::STANDARD.decode(attached.payload["replay_base64"].as_str().unwrap()).unwrap();
    let screen = String::from_utf8_lossy(&replay).replace(['\r', '\n'], " ");

    let dflow_dir = std::path::Path::new(env!("CARGO_BIN_EXE_dflowd")).parent().unwrap().to_string_lossy().to_lowercase();
    let low = screen.to_lowercase();
    println!("dflow dir expected on PATH: {dflow_dir}");
    // Show the PATH segment for the record.
    if let Some(i) = low.find("ppath=") {
        println!("PATH (first 300 chars): {}", &low[i..(i + 300).min(low.len())]);
    }
    // The echoed %Path% wraps across terminal rows, and a wrap boundary can split the
    // path with inserted whitespace, so compare with all whitespace removed (the paths
    // being matched contain no legitimate whitespace of their own).
    let squash = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
    assert!(
        squash(&low).contains(&squash(&dflow_dir)),
        "dflow binary dir not on the session PATH"
    );
}
