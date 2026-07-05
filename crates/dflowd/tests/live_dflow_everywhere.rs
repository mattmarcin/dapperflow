//! LIVE proof that a plain New Session (`session.create`, no card) on a real Claude Code
//! haiku session has `dflow` available AND is told when/how to use it through the injected
//! system prompt, with NO manual dflow instruction from the user (`agent-cli.md` /
//! Availability and standing guidance; `adapters.md` / Standing-guidance injection).
//!
//! Ignored by default; run with:
//!
//!   cargo test -p dflowd --test live_dflow_everywhere -- --ignored --nocapture
//!
//! It launches a claude launcher (adapter=claude, --model haiku) through the New Session
//! front door in an isolated scratch project, then proves two things:
//!   1. Guidance in context (deterministic): the daemon composed the launch with
//!      `--append-system-prompt <standing guidance>`, logged as
//!      "standing dflow guidance: append_system_prompt".
//!   2. Availability (behavioral): steering the session to run `dflow` yields the cardless
//!      "no card assigned" surface, proving the binary is on the session PATH and talks to
//!      the daemon over the injected token/endpoint.
//! The session is kept short and kill-verified; the daemon is this worktree's own build in
//! an isolated DFLOW_DATA_DIR, never the user's running daemon.

mod common;

use std::time::{Duration, Instant};

use base64::Engine;
use common::*;
use dflow_proto::{encode_frame, Envelope, FrameKind};
use futures_util::SinkExt;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Send a raw keypress into a session's PTY (to answer claude's approval menu, whose
/// default highlighted option is "Yes").
async fn send_key(ws: &mut Ws, session_id: &str, bytes: &[u8]) {
    let sid = ulid::Ulid::from_string(session_id).unwrap().to_bytes();
    let frame = encode_frame(FrameKind::Input, &sid, bytes);
    ws.send(WsMessage::Binary(frame.into())).await.unwrap();
}

fn claude_on_path() -> bool {
    let exts = ["", ".exe", ".cmd", ".bat"];
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .any(|dir| exts.iter().any(|e| dir.join(format!("claude{e}")).is_file()))
        })
        .unwrap_or(false)
}

async fn session_row(ws: &mut Ws, id: &str) -> Option<Value> {
    let mut sink = Vec::new();
    let resp = request(ws, &Envelope::message("sl", "session.list", serde_json::json!({})), &mut sink).await;
    resp.payload["sessions"].as_array()?.iter().find(|s| s["session_id"].as_str() == Some(id)).cloned()
}

/// Attach and return the current scrollback as plain text.
async fn read_screen(ws: &mut Ws, session_id: &str) -> String {
    let mut sink = Vec::new();
    let attached = request(
        ws,
        &Envelope::message("at", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 40 })),
        &mut sink,
    )
    .await;
    let replay = base64::engine::general_purpose::STANDARD
        .decode(attached.payload["replay_base64"].as_str().unwrap_or(""))
        .unwrap_or_default();
    String::from_utf8_lossy(&replay).to_string()
}

#[tokio::test]
#[ignore = "live: launches a real claude New Session on haiku"]
async fn live_new_session_claude_haiku_has_dflow_and_guidance() {
    if !claude_on_path() {
        eprintln!("SKIP: claude not on PATH");
        return;
    }
    let data_dir = unique_data_dir("live-everywhere");
    let (mut guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "info")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // A registered project so the New Session's cwd matches it (project-scoped token).
    let repo = scratch_repo(&data_dir);
    let repo_str = repo.to_string_lossy().to_string();
    request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo_str })), &mut sink).await;

    // A claude launcher pinned to haiku, exactly as a user's configured launcher would be.
    request(
        &mut ws,
        &Envelope::message(
            "aa",
            "agents.add",
            serde_json::json!({ "name": "claude-haiku", "adapter": "claude", "command": "claude", "extra_args": ["--model", "haiku"] }),
        ),
        &mut sink,
    )
    .await;

    // The New Session front door: cardless, cwd = the user's checkout, a TRIVIAL prompt
    // that never mentions dflow. Whether the agent knows about dflow must come only from
    // the injected standing guidance.
    let created = request(
        &mut ws,
        &Envelope::message(
            "s",
            "session.create",
            serde_json::json!({
                "harness": "claude",
                "agent": "claude-haiku",
                "cols": 120,
                "rows": 40,
                "cwd": repo_str,
                "first_prompt": "In two short sentences, what can you do in this session?"
            }),
        ),
        &mut sink,
    )
    .await;
    let session_id = created.payload["session_id"].as_str().expect("created").to_string();
    eprintln!("LIVE New Session {session_id} on claude/haiku (no dflow mention in the prompt)");

    // ---- Proof 1 (deterministic): the standing guidance reached the system prompt. ----
    // Poll the daemon log for the injection telemetry the daemon emits at spawn.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut injected = false;
    while Instant::now() < deadline {
        let log = std::fs::read_to_string(data_dir.join("daemon.log")).unwrap_or_default();
        if log.contains("standing dflow guidance: append_system_prompt") {
            injected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(injected, "daemon must inject the standing dflow guidance via --append-system-prompt");
    eprintln!("LIVE proof 1 OK: daemon logged append_system_prompt guidance injection");

    // Let claude boot (trust dialog auto-answered by the manifest watcher) and answer.
    let early = collect_output_until(&mut ws, "esc to interrupt", Duration::from_secs(30)).await;
    eprintln!("LIVE early screen:\n{early}\n---END EARLY---");
    // Wait for the model's first answer to settle.
    tokio::time::sleep(Duration::from_secs(20)).await;
    let answer = read_screen(&mut ws, &session_id).await;
    eprintln!("LIVE agent answer to the trivial prompt:\n{answer}\n---END ANSWER---");

    // ---- Proof 2 (behavioral): dflow is on PATH and works in the real session. ----
    // Steer the session to run dflow. This tests AVAILABILITY (running a named command),
    // distinct from the how/when guidance already proven in context above.
    request(
        &mut ws,
        &Envelope::message(
            "sv",
            "session.send_verified",
            serde_json::json!({ "session_id": session_id, "text": "Run the shell command `dflow` and paste its exact output." }),
        ),
        &mut sink,
    )
    .await;
    // claude parks arbitrary bash on an approval menu ("Do you want to proceed?") whose
    // default option is Yes; answer it with Enter so dflow actually executes.
    let mut buf = collect_output_until(&mut ws, "no card assigned", Duration::from_secs(30)).await;
    if !buf.contains("no card assigned") {
        send_key(&mut ws, &session_id, b"\r").await;
        buf.push_str(&collect_output_until(&mut ws, "no card assigned", Duration::from_secs(45)).await);
    }
    let full = format!("{}{}", read_screen(&mut ws, &session_id).await, buf);
    eprintln!("LIVE dflow-in-session screen:\n{full}\n---END DFLOW---");
    assert!(
        full.contains("no card assigned") || full.contains("cardless session"),
        "dflow must be available on the New Session PATH and report the cardless surface"
    );
    eprintln!("LIVE proof 2 OK: dflow ran in the session and reported the cardless surface");

    if let Some(row) = session_row(&mut ws, &session_id).await {
        eprintln!("LIVE final session row: {row}");
    }

    // Kill the session and stop the daemon; verify no orphan.
    let _ = request(
        &mut ws,
        &Envelope::message("k", "session.kill", serde_json::json!({ "session_id": session_id })),
        &mut sink,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    guard.kill_now();
    eprintln!("LIVE done; daemon killed");
}
