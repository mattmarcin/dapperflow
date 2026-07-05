//! `dflow` in every session, with automatic standing guidance
//! (`agent-cli.md` / Availability and standing guidance; `adapters.md` /
//! Standing-guidance injection; `security.md` / Per-task tokens).
//!
//! These drive the New Session front door (`session.create` with no card) exactly as the
//! desktop's Ctrl+N does, and prove the three contracts a plain session must satisfy so it
//! can keep the board current with no manual instruction:
//!
//! 1. Availability: the cardless session's spawn env carries `DFLOW_TOKEN` +
//!    `DFLOW_ENDPOINT` and the `dflow` dir is prepended to PATH, with `DFLOW_CARD` empty.
//! 2. Token scope: the cardless token is PROJECT-scoped - it may create cards, write
//!    knowledge, and self-report for its project, but may NOT move a foreign card, reach
//!    the vault, or kill a session.
//! 3. Cardless behavior + adoption: bare `dflow` reports the no-card state, and
//!    `dflow card create` files a card AND adopts it as the session's card going forward.

mod common;

use std::path::Path;
use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// A bare-terminal command that echoes the injected dflow env into the PTY (so the test
/// can read the real per-task token back and assert on the endpoint and empty card), then
/// runs `dflow` by BARE NAME - which resolves only if the dflow dir is on the session PATH,
/// and prints the cardless surface, proving availability without parsing the wrapped
/// `%PATH%` echo. `DCARD=[...]` brackets the card so an empty value is visible as `DCARD=[]`.
const ECHO_ENV: &str = r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%; DEND=%DFLOW_ENDPOINT%; DCARD=[%DFLOW_CARD%] & dflow"]"#;

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
    assert_eq!(welcome.payload["scope"], "agent", "cardless New Session token must be agent-scoped");
    ws
}

/// Start a cardless New Session in `cwd` whose command echoes its injected dflow env, then
/// read the session's scrollback and return `(session_id, screen)`.
async fn start_cardless_session(root: &mut Ws, sink: &mut Vec<u8>, cwd: &str) -> (String, String) {
    let created = request(
        root,
        &Envelope::message(
            "s",
            "session.create",
            serde_json::json!({
                "harness": "cmd",
                "command": serde_json::from_str::<serde_json::Value>(ECHO_ENV).unwrap(),
                "cols": 120,
                "rows": 32,
                "cwd": cwd,
            }),
        ),
        sink,
    )
    .await;
    assert_eq!(created.msg_type, "session.create", "create response: {created:?}");
    let session_id = created.payload["session_id"].as_str().unwrap().to_string();

    // Give the shell a moment to echo the injected env, then attach and read scrollback.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(
        root,
        &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })),
        sink,
    )
    .await;
    let replay = base64::engine::general_purpose::STANDARD
        .decode(attached.payload["replay_base64"].as_str().unwrap())
        .unwrap();
    (session_id, String::from_utf8_lossy(&replay).replace(['\r', '\n'], " "))
}

/// Extract the 48-char alphanumeric token following `DTOKEN=` in the screen text.
fn token_after(screen: &str, label: &str) -> String {
    let start = screen.find(label).map(|i| i + label.len()).expect("label present");
    let token: String = screen[start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    assert!(token.len() >= 40, "token too short after {label}: {screen}");
    token
}

/// One session's fleet row by id via `session.list`.
async fn fleet_row(ws: &mut Ws, sink: &mut Vec<u8>, session_id: &str) -> serde_json::Value {
    let resp = request(ws, &Envelope::message("l", "session.list", serde_json::json!({})), sink).await;
    resp.payload["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == session_id)
        .expect("session row present")
        .clone()
}

#[tokio::test]
async fn new_session_injects_project_scoped_dflow_env() {
    let data_dir = unique_data_dir("everywhere-env");
    let repo = scratch_repo(&data_dir);
    let repo_str = repo.to_string_lossy().to_string();

    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Register the project so the New Session's cwd matches it (cwd -> project).
    let padd = request(
        &mut root,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo_str })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    // ---- 1. Availability: DFLOW_TOKEN + DFLOW_ENDPOINT + dflow on PATH, empty card. ----
    let (session_id, mut screen) = start_cardless_session(&mut root, &mut sink, &repo_str).await;
    // The `& dflow` tail runs dflow by bare name once the token/endpoint/PATH are injected;
    // wait for its output to land in scrollback if it has not already.
    if !screen.contains("no card assigned") {
        screen.push_str(&collect_output_until(&mut root, "no card assigned", Duration::from_secs(15)).await);
    }
    let task_token = token_after(&screen, "DTOKEN=");
    assert!(screen.contains("DEND=ws://127.0.0.1"), "endpoint not injected: {screen}");
    // A cardless session's DFLOW_CARD is empty; Windows drops an empty-valued env var, so
    // cmd echoes the unexpanded `%DFLOW_CARD%` - either form means "no card" (and the CLI
    // treats unset and empty identically). The agent.context assertion below is the
    // authoritative proof that the token carries no card.
    assert!(
        screen.contains("DCARD=[]") || screen.contains("DCARD=[%DFLOW_CARD%]"),
        "cardless session must ship no DFLOW_CARD: {screen}"
    );
    // dflow is on the session PATH: running it by BARE NAME (the `& dflow` tail) resolves
    // and prints the cardless surface - a short, wrap-immune string - proving PATH
    // availability end-to-end without parsing the wrapped %PATH% echo.
    assert!(
        screen.contains("no card assigned"),
        "dflow must resolve by bare name on the New Session PATH and report the cardless surface: {screen}"
    );

    // ---- 2. Token scope: project-scoped agent token. ----
    let mut agent = connect_agent(port, &task_token).await;

    // Context resolves the PROJECT (cwd match) with no card yet.
    let ctx = request(&mut agent, &Envelope::message("x", "agent.context", serde_json::json!({})), &mut sink).await;
    assert!(ctx.payload["card"].is_null(), "a fresh cardless session has no card: {:?}", ctx.payload);
    assert_eq!(ctx.payload["project_name"], "repo", "token must be scoped to the cwd's project");

    // It MAY create a card (project-scoped card-create) ...
    let made = request(
        &mut agent,
        &Envelope::message("cc", "card.create", serde_json::json!({ "title": "found a bug while exploring", "type": "bug" })),
        &mut sink,
    )
    .await;
    assert_eq!(made.msg_type, "card.create", "cardless token must be allowed to create a card: {made:?}");
    let my_card = made.payload["card_id"].as_str().unwrap().to_string();
    assert_eq!(made.payload["card"]["project_id"], project_id, "created card lands in the token's project");

    // ---- 3. Adoption: the created card becomes the session's card. ----
    let row = fleet_row(&mut root, &mut sink, &session_id).await;
    assert_eq!(row["card_id"], my_card, "dflow card create must set the session's card");
    let ctx2 = request(&mut agent, &Envelope::message("x2", "agent.context", serde_json::json!({})), &mut sink).await;
    assert_eq!(ctx2.payload["card"]["id"], my_card, "bare dflow now resolves the adopted card");

    // ---- Scope NEGATIVES: a foreign card, the vault, and killing sessions are Forbidden. ----
    // A card the token did not create (filed by root) may not be moved by this token.
    let foreign = request(
        &mut root,
        &Envelope::message("fc", "card.create", serde_json::json!({ "title": "someone else's card", "type": "feature", "project_id": project_id })),
        &mut sink,
    )
    .await;
    let foreign_card = foreign.payload["card_id"].as_str().unwrap().to_string();
    let denied = request(
        &mut agent,
        &Envelope::message("mv", "card.move", serde_json::json!({ "card_id": foreign_card, "column": "performing" })),
        &mut sink,
    )
    .await;
    assert_eq!(denied.msg_type, "error", "moving a foreign card must be refused: {denied:?}");
    assert_eq!(denied.payload["code"], "forbidden");

    // The vault is owner-only: an agent token cannot read or write env entries.
    let vault = request(
        &mut agent,
        &Envelope::message("ev", "env.list", serde_json::json!({ "project_id": project_id })),
        &mut sink,
    )
    .await;
    assert_eq!(vault.msg_type, "error", "vault must be outside the cardless token scope: {vault:?}");
    assert_eq!(vault.payload["code"], "forbidden");

    // Killing a session is owner-only.
    let kill = request(
        &mut agent,
        &Envelope::message("kl", "session.kill", serde_json::json!({ "session_id": session_id })),
        &mut sink,
    )
    .await;
    assert_eq!(kill.msg_type, "error", "session.kill must be outside the cardless token scope: {kill:?}");
    assert_eq!(kill.payload["code"], "forbidden");

    let _ = request(&mut root, &Envelope::message("q", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}

#[tokio::test]
async fn cardless_dflow_binary_surface() {
    // The real `dflow` binary, run with a cardless project-scoped token and an EMPTY
    // DFLOW_CARD, must behave sensibly: bare `dflow` reports the no-card state, `status`
    // self-reports on the session, `card create` adopts a card, and `know` works.
    let dflow = Path::new(env!("CARGO_BIN_EXE_dflowd"))
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "dflow.exe" } else { "dflow" });
    if !dflow.exists() {
        eprintln!("skipping: {} not built (run `cargo test --workspace`)", dflow.display());
        return;
    }

    let data_dir = unique_data_dir("everywhere-bin");
    let repo = scratch_repo(&data_dir);
    let repo_str = repo.to_string_lossy().to_string();
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    request(&mut root, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo_str })), &mut sink).await;
    let (session_id, screen) = start_cardless_session(&mut root, &mut sink, &repo_str).await;
    let task_token = token_after(&screen, "DTOKEN=");
    let endpoint = format!("ws://127.0.0.1:{port}/ws");

    // Run the real binary with an EMPTY DFLOW_CARD, exactly as a New Session ships it.
    let run = |args: &[&str]| {
        std::process::Command::new(&dflow)
            .args(args)
            .env("DFLOW_TOKEN", &task_token)
            .env("DFLOW_ENDPOINT", &endpoint)
            .env("DFLOW_CARD", "")
            .output()
            .expect("run dflow")
    };

    // Bare `dflow`: the definitive no-card state plus a next step, exit 0.
    let out = run(&[]);
    assert!(out.status.success(), "bare dflow exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("no card assigned"), "cardless dflow should say no card: {stdout}");
    assert!(stdout.contains("next:"), "cardless dflow needs a next line: {stdout}");

    // `dflow status working <note>`: self-reports on the session (no card needed), exit 0.
    let out = run(&["status", "working", "exploring the repo"]);
    assert!(out.status.success(), "status working failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("recorded: working"));
    let row = fleet_row(&mut root, &mut sink, &session_id).await;
    assert_eq!(row["state"], "working", "cardless self-report updates the session row");
    assert_eq!(row["status_note"], "exploring the repo");

    // `dflow card create` files a card AND adopts it as the session's card going forward.
    let out = run(&["card", "create", "--title", "wire the thing", "--type", "feature"]);
    assert!(out.status.success(), "card create failed: {}", String::from_utf8_lossy(&out.stderr));
    let created = String::from_utf8_lossy(&out.stdout);
    assert!(created.contains("created card"), "card create output: {created}");
    let row = fleet_row(&mut root, &mut sink, &session_id).await;
    let adopted = row["card_id"].as_str().expect("session adopted its created card");

    // Now bare `dflow` resolves the adopted card instead of the no-card state.
    let out = run(&[]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("card {adopted}")), "bare dflow should show the adopted card: {stdout}");

    // `dflow know` verbs are project-scoped and work with no card.
    let out = run(&["know", "add", "--type", "gotcha", "--title", "cardless note", "--file", &repo.join("README.md").to_string_lossy()]);
    assert!(out.status.success(), "know add: {}", String::from_utf8_lossy(&out.stderr));
    assert!(repo.join("docs/knowledge/gotchas/cardless-note.md").exists(), "know add writes to the project knowledgebase");
    let out = run(&["know", "find", "cardless"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("gotchas/cardless-note"), "know find must locate the note");

    let _ = request(&mut root, &Envelope::message("q", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}
