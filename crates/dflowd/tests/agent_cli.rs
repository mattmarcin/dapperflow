//! End-to-end agent-CLI surface tests (`agent-cli.md`, `knowledge.md`, `security.md`).
//!
//! These drive the daemon exactly as the `dflow` binary does: a dispatched stub
//! session mints a per-task token and injects it into the session env; the test reads
//! that real token back from the session scrollback, then opens its own `agent` WS
//! connection with it and exercises every scoped verb. This proves token injection,
//! scope enforcement, tier-1 arbitration, the knowledge round-trip, and budgets against
//! the real daemon, deterministically and offline.

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The stub echoes its injected DFLOW_TOKEN and endpoint into the PTY, then stays alive
/// so the test can attach and read the token back.
const STUB_LAUNCH: &str = r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%; DEND=%DFLOW_ENDPOINT%; DCARD=%DFLOW_CARD%"]"#;

/// Open an `agent`-client WS connection authenticated with a per-task token.
async fn connect_agent(port: u16, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let hello = Envelope::message(
        "auth",
        "auth.hello",
        AuthHello { token: token.to_string(), client: ClientKind::Agent, proto_versions: vec![PROTOCOL_VERSION] },
    );
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let welcome = next_envelope(&mut ws).await;
    assert_eq!(welcome.msg_type, "auth.welcome", "agent auth failed: {welcome:?}");
    assert_eq!(welcome.payload["scope"], "agent", "expected an agent-scoped welcome");
    ws
}

#[tokio::test]
async fn agent_cli_surface_end_to_end() {
    let data_dir = unique_data_dir("agentcli");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Register the project and file the dispatch card.
    let padd = request(
        &mut root,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    let cadd = request(
        &mut root,
        &Envelope::message(
            "c",
            "card.create",
            serde_json::json!({ "title": "wire the login flow", "type": "feature", "project_id": project_id,
                "brief": "Do the thing.\n\n## Acceptance\n- login persists\n- errors surface\n" }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Dispatch the stub with a card budget of 2 and a note budget of 1.
    let disp = request(
        &mut root,
        &Envelope::message(
            "d",
            "dispatch.start",
            serde_json::json!({ "card_id": card_id, "harness": "stub", "budget_cards": 2, "budget_notes": 1 }),
        ),
        &mut sink,
    )
    .await;
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    // Give the stub a moment to echo its injected env, then read the real task token
    // from the session's scrollback (exactly what dispatch injected).
    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(
        &mut root,
        &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })),
        &mut sink,
    )
    .await;
    let replay = base64::engine::general_purpose::STANDARD
        .decode(attached.payload["replay_base64"].as_str().unwrap())
        .unwrap();
    let screen = String::from_utf8_lossy(&replay);
    let task_token = extract_token(&screen)
        .unwrap_or_else(|| panic!("DFLOW_TOKEN not found in session scrollback: {screen:?}"));
    assert!(screen.contains(&format!("DCARD={card_id}")), "DFLOW_CARD not injected: {screen:?}");

    // ---- The agent connects with its per-task token. ----
    let mut agent = connect_agent(port, &task_token).await;

    // agent.context resolves the card, project, and acceptance from the token.
    let ctx = request(&mut agent, &Envelope::message("x1", "agent.context", serde_json::json!({})), &mut sink).await;
    assert_eq!(ctx.payload["card"]["id"], card_id);
    assert_eq!(ctx.payload["project_name"], "repo"); // the scratch repo's dir name
    let acceptance = ctx.payload["acceptance"].as_array().unwrap();
    assert_eq!(acceptance.len(), 2, "acceptance parsed from the brief: {acceptance:?}");
    assert_eq!(acceptance[0], "login persists");

    // ---- Tier-1 self-report: working with a note updates state + status_note. ----
    let sr = request(
        &mut agent,
        &Envelope::message("x2", "session.self_report", serde_json::json!({ "state": "working", "note": "wiring reducer" })),
        &mut sink,
    )
    .await;
    assert_eq!(sr.payload["recorded"], "working");
    // Verify via a root session.list that the strip note landed.
    let listing = request(&mut root, &Envelope::message("l", "session.list", serde_json::json!({})), &mut sink).await;
    let row = listing.payload["sessions"].as_array().unwrap().iter().find(|s| s["session_id"] == session_id).unwrap();
    assert_eq!(row["state"], "working");
    assert_eq!(row["status_note"], "wiring reducer");

    // blocked requires a note; without one it is a bad request (usage error client-side).
    let bad = request(&mut agent, &Envelope::message("x3", "session.self_report", serde_json::json!({ "state": "blocked" })), &mut sink).await;
    assert_eq!(bad.msg_type, "error");
    assert_eq!(bad.payload["code"], "bad_request");

    // ---- Knowledge round-trip: add -> find -> get -> catalog + index. ----
    let add = request(
        &mut agent,
        &Envelope::message("k1", "know.add", serde_json::json!({ "type": "gotcha", "title": "ConPTY resize storm", "body": "Debounce resize on Windows.\nA second line.", "tags": ["windows", "pty"] })),
        &mut sink,
    )
    .await;
    assert_eq!(add.payload["id"], "gotchas/conpty-resize-storm");
    assert_eq!(add.payload["created"], true);
    // The note landed in the PROJECT ROOT checkout, not the worktree.
    let note_path = repo.join("docs/knowledge/gotchas/conpty-resize-storm.md");
    assert!(note_path.exists(), "note not written to project root: {}", note_path.display());
    // The Catalog was regenerated in index.md.
    let index = std::fs::read_to_string(repo.join("docs/knowledge/index.md")).unwrap();
    assert!(index.contains("[[gotchas/conpty-resize-storm]]"), "catalog not regenerated: {index}");

    let idx = request(&mut agent, &Envelope::message("k2", "know.index", serde_json::json!({})), &mut sink).await;
    assert_eq!(idx.payload["total_notes"], 1);

    let found = request(&mut agent, &Envelope::message("k3", "know.find", serde_json::json!({ "query": "resize" })), &mut sink).await;
    let hits = found.payload["notes"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["id"], "gotchas/conpty-resize-storm");

    let got = request(&mut agent, &Envelope::message("k4", "know.get", serde_json::json!({ "id": "gotchas/conpty-resize-storm" })), &mut sink).await;
    assert!(got.payload["note"]["body"].as_str().unwrap().contains("Debounce resize on Windows."));

    // A knowledge_updated event landed on the card timeline.
    let cget = request(&mut root, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    let kinds: Vec<&str> = cget.payload["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str()).collect();
    assert!(kinds.contains(&"knowledge_updated"), "no knowledge_updated event: {kinds:?}");

    // ---- Note budget (1) is exhausted: a second note is refused. ----
    let over = request(
        &mut agent,
        &Envelope::message("k5", "know.add", serde_json::json!({ "type": "note", "title": "second", "body": "x" })),
        &mut sink,
    )
    .await;
    assert_eq!(over.msg_type, "error");
    assert_eq!(over.payload["code"], "budget_exceeded");

    // ---- Card budget (2): two creates succeed, the third is refused. ----
    let mut created_ids = Vec::new();
    for i in 0..2 {
        let cc = request(
            &mut agent,
            &Envelope::message(format!("cc{i}"), "card.create", serde_json::json!({ "title": format!("follow up {i}"), "type": "bug" })),
            &mut sink,
        )
        .await;
        assert_eq!(cc.msg_type, "card.create", "create {i} failed: {cc:?}");
        created_ids.push(cc.payload["card_id"].as_str().unwrap().to_string());
    }
    let cc3 = request(&mut agent, &Envelope::message("cc3", "card.create", serde_json::json!({ "title": "one too many", "type": "bug" })), &mut sink).await;
    assert_eq!(cc3.msg_type, "error");
    assert_eq!(cc3.payload["code"], "budget_exceeded");

    // ---- Scope: the agent may move its own card, but not a foreign card. ----
    let mv = request(&mut agent, &Envelope::message("mv", "card.move", serde_json::json!({ "card_id": card_id, "column": "performing" })), &mut sink).await;
    assert_eq!(mv.msg_type, "card.move", "own-card move rejected: {mv:?}");

    // A card the token neither owns nor created (filed by root) is out of scope.
    let foreign = request(
        &mut root,
        &Envelope::message("fc", "card.create", serde_json::json!({ "title": "foreign", "type": "chore", "project_id": project_id })),
        &mut sink,
    )
    .await;
    let foreign_id = foreign.payload["card_id"].as_str().unwrap().to_string();
    let denied = request(&mut agent, &Envelope::message("fx", "card.move", serde_json::json!({ "card_id": foreign_id, "column": "done" })), &mut sink).await;
    assert_eq!(denied.msg_type, "error", "cross-card move must be rejected: {denied:?}");
    assert_eq!(denied.payload["code"], "forbidden");

    // ---- A fleet verb is not on the agent allowlist. ----
    let fleet = request(&mut agent, &Envelope::message("fl", "session.list", serde_json::json!({})), &mut sink).await;
    assert_eq!(fleet.msg_type, "error");
    assert_eq!(fleet.payload["code"], "forbidden");

    // ---- notify.forward (codex bridge) applies a turn-complete to the session. ----
    let nf = request(
        &mut agent,
        &Envelope::message("nf", "notify.forward", serde_json::json!({ "payload": "{\"type\":\"agent-turn-complete\",\"thread-id\":\"thread-xyz\"}" })),
        &mut sink,
    )
    .await;
    assert_eq!(nf.msg_type, "notify.forward", "notify.forward failed: {nf:?}");
    // The thread id was captured into resume_ref; the turn-complete drove idle.
    let listing2 = request(&mut root, &Envelope::message("l2", "session.list", serde_json::json!({})), &mut sink).await;
    let row2 = listing2.payload["sessions"].as_array().unwrap().iter().find(|s| s["session_id"] == session_id).unwrap();
    assert_eq!(row2["resume_ref"], "thread-xyz", "codex thread-id not captured: {row2:?}");

    // ---- A bogus task token is rejected at auth (the CLI maps this to exit 4). ----
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut bogus, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = Envelope::message("auth", "auth.hello", AuthHello { token: "not-a-real-token".into(), client: ClientKind::Agent, proto_versions: vec![PROTOCOL_VERSION] });
    bogus.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let closed = matches!(
        tokio::time::timeout(Duration::from_secs(5), futures_util::StreamExt::next(&mut bogus)).await,
        Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None)
    );
    assert!(closed, "a bogus agent token must be rejected at auth");
}

/// Drive the REAL `dflow` binary (built alongside `dflowd`) as a subprocess against
/// the live daemon, proving the executable - not just the protocol - works end to end.
/// Skips gracefully if `dflow` was not built (run under `cargo test --workspace`).
#[tokio::test]
async fn real_dflow_binary_against_daemon() {
    let dflow = std::path::Path::new(env!("CARGO_BIN_EXE_dflowd"))
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "dflow.exe" } else { "dflow" });
    if !dflow.exists() {
        eprintln!("skipping: {} not built (run `cargo test --workspace`)", dflow.display());
        return;
    }

    let data_dir = unique_data_dir("dflowbin");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut root, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(&mut root, &Envelope::message("c", "card.create", serde_json::json!({ "title": "binary smoke", "type": "chore", "project_id": project_id })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();
    let disp = request(&mut root, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(&mut root, &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })), &mut sink).await;
    let replay = base64::engine::general_purpose::STANDARD.decode(attached.payload["replay_base64"].as_str().unwrap()).unwrap();
    let task_token = extract_token(&String::from_utf8_lossy(&replay)).expect("token in scrollback");
    let endpoint = format!("ws://127.0.0.1:{port}/ws");

    let run = |args: &[&str]| {
        std::process::Command::new(&dflow)
            .args(args)
            .env("DFLOW_TOKEN", &task_token)
            .env("DFLOW_ENDPOINT", &endpoint)
            .env("DFLOW_CARD", &card_id)
            .output()
            .expect("run dflow")
    };

    // Bare `dflow`: the card headline and a next line, exit 0.
    let out = run(&[]);
    assert!(out.status.success(), "bare dflow exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("card {card_id}")), "bare dflow output: {stdout}");
    assert!(stdout.contains("next:"), "no next line: {stdout}");

    // `dflow status working <note>`: recorded working, exit 0.
    let out = run(&["status", "working", "binary-note"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("recorded: working"));
    // The daemon really updated the session strip note.
    let listing = request(&mut root, &Envelope::message("l", "session.list", serde_json::json!({})), &mut sink).await;
    let row = listing.payload["sessions"].as_array().unwrap().iter().find(|s| s["session_id"] == session_id).unwrap();
    assert_eq!(row["status_note"], "binary-note");

    // `dflow status blocked` with no note is a usage error, exit 2 (client-side).
    let out = run(&["status", "blocked"]);
    assert_eq!(out.status.code(), Some(2), "blocked-without-note should exit 2");

    // `dflow know add` writes a real note; `dflow know find` finds it.
    let out = run(&["know", "add", "--type", "gotcha", "--title", "binary gotcha", "--file", &repo.join("README.md").to_string_lossy()]);
    assert!(out.status.success(), "know add: {}", String::from_utf8_lossy(&out.stderr));
    assert!(repo.join("docs/knowledge/gotchas/binary-gotcha.md").exists());
    let out = run(&["know", "find", "binary"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("gotchas/binary-gotcha"));

    // No DFLOW_TOKEN in the environment: exit 3 (not in a dispatched context).
    let out = std::process::Command::new(&dflow)
        .arg("status").arg("working")
        .env_remove("DFLOW_TOKEN")
        .output()
        .expect("run dflow");
    assert_eq!(out.status.code(), Some(3), "missing token should exit 3");
}

/// Extract the 48-char alphanumeric token following `DTOKEN=` in the screen text.
fn extract_token(screen: &str) -> Option<String> {
    let start = screen.find("DTOKEN=")? + "DTOKEN=".len();
    let token: String = screen[start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    if token.len() >= 40 {
        Some(token)
    } else {
        None
    }
}
