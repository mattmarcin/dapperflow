//! Dispatch-level M3 tests against a real daemon: the scripted stub planning agent
//! driving the real `dflow` CLI plan loop, the per-worktree port broker + service
//! lifecycle + required-failure parking, the drift guard on worktree return, and the
//! artifact secret scan (`plan-studio.md`, `environments.md`, `security.md`).

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The path to the built `dflow` CLI next to `dflowd` (dispatch puts it on PATH).
fn dflow_binary() -> PathBuf {
    let dflowd = PathBuf::from(env!("CARGO_BIN_EXE_dflowd"));
    let name = if cfg!(windows) { "dflow.exe" } else { "dflow" };
    dflowd.parent().unwrap().join(name)
}

/// Open an `agent`-client WS connection with a per-task token.
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
    ws
}

fn extract_after(screen: &str, marker: &str) -> Option<String> {
    // The raw PTY replay carries ANSI escapes; take only the leading token/port charset
    // ([A-Za-z0-9._-]) after the marker so a trailing escape sequence is not captured.
    let tail = screen.split(marker).nth(1)?;
    let value: String = tail
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .collect();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Attach to a session repeatedly until its scrollback contains `marker`, returning the
/// value that follows it. Robust to a slow shell cold-start (this machine's `cmd` alone
/// takes ~3s to draw its first output).
async fn read_after_marker(root: &mut Ws, session_id: &str, marker: &str, timeout: Duration) -> String {
    let mut sink = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        let attached = request(root, &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })), &mut sink).await;
        let replay = base64::engine::general_purpose::STANDARD.decode(attached.payload["replay_base64"].as_str().unwrap()).unwrap();
        let screen = String::from_utf8_lossy(&replay);
        if let Some(v) = extract_after(&screen, marker) {
            return v;
        }
        assert!(Instant::now() < deadline, "marker {marker:?} never appeared in scrollback: {screen:?}");
        let _ = request(root, &Envelope::message("dt", "session.detach", serde_json::json!({ "session_id": session_id })), &mut sink).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

/// Set up a project + carded work item, returning `(root ws, project_id, card_id)`.
async fn project_and_card(root: &mut Ws, repo: &std::path::Path, title: &str) -> (String, String) {
    let mut sink = Vec::new();
    let padd = request(
        root,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(
        root,
        &Envelope::message("c", "card.create", serde_json::json!({ "title": title, "type": "feature", "project_id": project_id })),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();
    (project_id, card_id)
}

// ---------------------------------------------------------------------------
// The scripted stub planning agent: drives `dflow plan open` / `dflow plan poll`
// end to end over the real CLI, no LLM (REQUIRED deliverable).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stub_planning_agent_drives_the_dflow_plan_loop() {
    let dflow = dflow_binary();
    if !dflow.exists() {
        eprintln!("SKIP: dflow CLI not built next to dflowd ({}); `cargo build -p dflow-cli` first (built automatically under `cargo test --workspace`)", dflow.display());
        return;
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stub_planning_agent.ps1");
    let launch = serde_json::json!([
        "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script.to_string_lossy()
    ])
    .to_string();

    let data_dir = unique_data_dir("stubplan");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", launch.as_str())]);

    let mut ctrl = connect_and_auth(port, &token).await;
    let mut events = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let (_project_id, card_id) = project_and_card(&mut ctrl, &repo, "retry policy plan").await;

    // Subscribe to the event stream BEFORE dispatch so no round is missed.
    let _sub = request(&mut events, &Envelope::message("sub", "event.subscribe", serde_json::json!({})), &mut sink).await;

    // Dispatch the stub planning agent (harness "stub" -> the powershell plan loop).
    let disp = request(
        &mut ctrl,
        &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })),
        &mut sink,
    )
    .await;
    assert_eq!(disp.msg_type, "dispatch.start", "dispatch failed: {disp:?}");
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    // Drive the human side reactively from the event stream: round 1 -> feedback,
    // round 2 -> approve. Assert the session parks in awaiting_feedback while the agent
    // is on `dflow plan poll`.
    let mut artifact_id = String::new();
    let mut saw_awaiting = false;
    let mut approved = false;
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline && !approved {
        let ev = tokio::time::timeout(Duration::from_secs(60), next_envelope(&mut events))
            .await
            .expect("timed out waiting for a plan event");
        if ev.msg_type != "event.card_event" {
            continue;
        }
        let e = &ev.payload["event"];
        match e["kind"].as_str().unwrap_or("") {
            "artifact_opened" => {
                artifact_id = e["payload"]["artifact_id"].as_str().unwrap_or("").to_string();
            }
            "plan_round" => {
                let round = e["payload"]["round"].as_u64().unwrap_or(0);
                if artifact_id.is_empty() {
                    artifact_id = e["payload"]["artifact_id"].as_str().unwrap_or("").to_string();
                }
                if round == 1 {
                    // The agent opened and is polling: it should park in awaiting_feedback.
                    saw_awaiting |= wait_for_state(&mut ctrl, &session_id, "awaiting_feedback", Duration::from_secs(20)).await;
                    submit_feedback(&mut ctrl, &artifact_id, 1).await;
                } else {
                    submit_approve(&mut ctrl, &artifact_id, round).await;
                }
            }
            "plan_approved" => approved = true,
            _ => {}
        }
    }

    assert!(approved, "the plan loop should reach approval; artifact={artifact_id}");
    assert!(saw_awaiting, "the session must park in awaiting_feedback while the agent polls");

    // The artifact is approved on the card.
    let cget = request(&mut ctrl, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    let artifacts = cget.payload["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["status"], "approved");
    assert!(artifacts[0]["round"].as_u64().unwrap_or(0) >= 2, "at least two review rounds ran");

    // The stub's log proves the real `dflow` CLI drove the loop.
    let log = read_agent_log(disp.payload["worktree_path"].as_str().unwrap(), Duration::from_secs(15));
    assert!(log.contains("AGENT open"), "the stub ran `dflow plan open`: {log}");
    assert!(log.contains("AGENT poll"), "the stub ran `dflow plan poll`: {log}");
    assert!(
        log.contains("review approved") || log.contains("AGENT done"),
        "the stub's poll saw approval: {log}"
    );
}

/// Wait until the stub writes and completes its log (best-effort, bounded).
fn read_agent_log(worktree_path: &str, timeout: Duration) -> String {
    let log = PathBuf::from(worktree_path).join("agent.log");
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(text) = std::fs::read_to_string(&log) {
            if text.contains("AGENT exiting") || text.contains("AGENT done") {
                return text;
            }
        }
        if Instant::now() >= deadline {
            return std::fs::read_to_string(&log).unwrap_or_default();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Poll session.list until the session reaches `want` state, or timeout.
async fn wait_for_state(ctrl: &mut Ws, session_id: &str, want: &str, timeout: Duration) -> bool {
    let mut sink = Vec::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let listing = request(ctrl, &Envelope::message("l", "session.list", serde_json::json!({})), &mut sink).await;
        if let Some(row) = listing.payload["sessions"].as_array().and_then(|a| a.iter().find(|s| s["session_id"] == session_id)) {
            if row["state"] == want {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

async fn submit_feedback(ctrl: &mut Ws, artifact_id: &str, round: u64) {
    let mut sink = Vec::new();
    let r = request(
        ctrl,
        &Envelope::message(
            "sub",
            "artifact.feedback.submit",
            serde_json::json!({
                "artifact_id": artifact_id,
                "round": round,
                "items": [
                    { "kind": "text_range", "anchor": { "selector": "#retry", "start": 0, "end": 30, "quote": "retry with exponential backoff" }, "body": "cap at 3 attempts", "status": "anchored" },
                    { "kind": "control", "question_key": "storage", "value": "sqlite" }
                ]
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(r.payload["ok"], true, "feedback submit failed: {r:?}");
}

async fn submit_approve(ctrl: &mut Ws, artifact_id: &str, round: u64) {
    let mut sink = Vec::new();
    let r = request(
        ctrl,
        &Envelope::message(
            "approve",
            "artifact.feedback.submit",
            serde_json::json!({
                "artifact_id": artifact_id,
                "round": round,
                "items": [ { "kind": "action", "action": "approve_plan" } ]
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(r.payload["ok"], true, "approve failed: {r:?}");
}

// ---------------------------------------------------------------------------
// Port broker: a per-worktree service's allocated port is injected as
// DFLOW_PORT_<NAME> into the agent's spawn env, and its lifecycle is recorded.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn port_broker_injects_service_ports_into_the_agent() {
    // The stub echoes the injected DFLOW_PORT_HTTP and its token so the test can read
    // both back from the session scrollback.
    let launch = r#"["cmd.exe","/d","/k","echo DPORT=%DFLOW_PORT_HTTP%; DTOKEN=%DFLOW_TOKEN%"]"#;
    let data_dir = unique_data_dir("portbroker");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", launch)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let (project_id, card_id) = project_and_card(&mut root, &repo, "dev server").await;

    // Declare a long-lived per-worktree service that binds a broker-allocated port.
    let svc_cmd = if cfg!(windows) {
        "ping -n 30 127.0.0.1 -w {DFLOW_PORT_HTTP}"
    } else {
        "sleep 30 {DFLOW_PORT_HTTP}"
    };
    let sadd = request(
        &mut root,
        &Envelope::message(
            "svc",
            "service.add",
            serde_json::json!({ "project_id": project_id, "name": "web", "cmd": svc_cmd, "ports": ["HTTP"], "required": true }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(sadd.msg_type, "service.add", "service.add failed: {sadd:?}");

    // Dispatch: the broker allocates a free port and injects DFLOW_PORT_HTTP.
    let disp = request(&mut root, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    assert_eq!(disp.msg_type, "dispatch.start", "dispatch failed: {disp:?}");
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    let dport = read_after_marker(&mut root, &session_id, "DPORT=", Duration::from_secs(20)).await;
    let alloc: u16 = dport.trim().parse().unwrap_or_else(|_| panic!("DFLOW_PORT_HTTP not injected as a port: {dport:?}"));
    assert!(alloc > 0, "the port broker allocated a real free port: {alloc}");

    // The service lifecycle is on the card timeline.
    let cget = request(&mut root, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 200 })), &mut sink).await;
    let events = cget.payload["events"].as_array().unwrap();
    let started = events.iter().find(|e| e["kind"] == "service_started").expect("service_started event recorded");
    assert_eq!(started["payload"]["service"], "web");
    assert_eq!(started["payload"]["ports"]["HTTP"], alloc);

    // Teardown stops the service (its whole tree dies with the Job Object).
    let _ = request(&mut root, &Envelope::message("cancel", "dispatch.cancel", serde_json::json!({ "card_id": card_id })), &mut sink).await;
}

// ---------------------------------------------------------------------------
// A required service that fails its health check parks the card and aborts dispatch.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn required_service_failure_parks_the_card() {
    // A generous settle window observes the exit even on this machine's slow shell start.
    // The harness must still resolve (dispatch resolves it before starting services), so
    // an idle stub launch stands in; it is never actually launched because the required
    // service fails first.
    let launch = r#"["cmd.exe","/d","/k","echo READY"]"#;
    let data_dir = unique_data_dir("svcfail");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(
        &data_dir,
        &[("DFLOW_SERVICE_HEALTH_SETTLE_MS", "7000"), ("DFLOW_LAUNCH_STUB", launch)],
    );
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let (project_id, card_id) = project_and_card(&mut root, &repo, "broken backend").await;

    request(
        &mut root,
        &Envelope::message(
            "svc",
            "service.add",
            serde_json::json!({ "project_id": project_id, "name": "backend", "cmd": "exit 1", "required": true }),
        ),
        &mut sink,
    )
    .await;

    // Dispatch must fail (the agent is not launched) and park the card.
    let disp = request(&mut root, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    assert_eq!(disp.msg_type, "error", "dispatch must fail on a required service failure: {disp:?}");
    assert!(disp.payload["message"].as_str().unwrap_or("").contains("backend"), "the error names the failed service: {disp:?}");

    // A service_failed Needs You item is raised and a service_failed event recorded.
    let fleet = request(&mut root, &Envelope::message("f", "fleet.status", serde_json::json!({})), &mut sink).await;
    let needs = fleet.payload["needs_you"].as_array().unwrap();
    assert!(needs.iter().any(|n| n["kind"] == "service_failed" && n["card_id"] == card_id), "a service_failed Needs You item is raised: {needs:?}");

    let cget = request(&mut root, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 200 })), &mut sink).await;
    let events = cget.payload["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["kind"] == "service_failed" && e["payload"]["service"] == "backend"), "a service_failed event is recorded: {events:?}");
    // No session was created for this card.
    assert!(cget.payload["sessions"].as_array().unwrap().is_empty(), "the agent must not launch");
}

// ---------------------------------------------------------------------------
// Drift guard: an edited materialized env file raises a value-masked env_drift on return.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn env_drift_raised_on_worktree_return() {
    let launch = r#"["cmd.exe","/d","/k","echo READY"]"#;
    let data_dir = unique_data_dir("drift");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", launch)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let (project_id, card_id) = project_and_card(&mut root, &repo, "env work").await;

    // A materialized `file` vault entry.
    request(
        &mut root,
        &Envelope::message(
            "env",
            "env.set",
            serde_json::json!({ "project_id": project_id, "key": "dev-vars", "kind": "file", "target": ".dev.vars", "value": "API_KEY=old\nMODE=dev\n" }),
        ),
        &mut sink,
    )
    .await;

    let disp = request(&mut root, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let worktree_path = disp.payload["worktree_path"].as_str().unwrap().to_string();

    // The materialized file exists; the agent edits it (add a key, change one).
    let file = PathBuf::from(&worktree_path).join(".dev.vars");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !file.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(file.exists(), "the vault file must be materialized into the worktree");
    std::fs::write(&file, "API_KEY=changed\nMODE=dev\nEXTRA=added\n").unwrap();

    // Return the worktree: drift is detected before shredding.
    let _ = request(&mut root, &Envelope::message("cancel", "dispatch.cancel", serde_json::json!({ "card_id": card_id })), &mut sink).await;

    let cget = request(&mut root, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 200 })), &mut sink).await;
    let events = cget.payload["events"].as_array().unwrap();
    let drift = events.iter().find(|e| e["kind"] == "env_drift").expect("an env_drift event is recorded");
    assert_eq!(drift["payload"]["target"], ".dev.vars");
    // The summary carries key names only (masked): API_KEY changed, EXTRA added.
    let changed = drift["payload"]["changed_keys"].as_array().unwrap();
    assert!(changed.iter().any(|k| k == "API_KEY"), "API_KEY change detected: {drift:?}");
    let added = drift["payload"]["added_keys"].as_array().unwrap();
    assert!(added.iter().any(|k| k == "EXTRA"), "EXTRA addition detected: {drift:?}");
    let debug = drift.to_string();
    assert!(!debug.contains("changed") || !debug.contains("\"changed_value\""), "no raw values in the summary");
    assert!(!debug.contains("API_KEY=changed"), "the drift summary must mask values: {debug}");

    // A value-masked env_drift Needs You item is raised.
    let fleet = request(&mut root, &Envelope::message("f", "fleet.status", serde_json::json!({})), &mut sink).await;
    let needs = fleet.payload["needs_you"].as_array().unwrap();
    assert!(needs.iter().any(|n| n["kind"] == "env_drift" && n["card_id"] == card_id), "an env_drift Needs You item is raised: {needs:?}");
}

// ---------------------------------------------------------------------------
// The artifact secret scan refuses an agent artifact that contains a known secret value.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_artifact_secret_scan_refuses_registration() {
    let launch = r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%"]"#;
    let data_dir = unique_data_dir("secretscan");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", launch)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let (project_id, card_id) = project_and_card(&mut root, &repo, "leak check").await;

    // A materialized secret whose value must never appear in an artifact.
    let secret_value = "sk-super-secret-abc123-DO-NOT-LEAK";
    request(
        &mut root,
        &Envelope::message(
            "env",
            "env.set",
            serde_json::json!({ "project_id": project_id, "key": "API_KEY", "kind": "secret", "value": secret_value }),
        ),
        &mut sink,
    )
    .await;

    let disp = request(&mut root, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();
    let worktree_path = disp.payload["worktree_path"].as_str().unwrap().to_string();

    // Read the injected task token from scrollback (retrying past the slow shell start)
    // and connect as the agent.
    let task_token = read_after_marker(&mut root, &session_id, "DTOKEN=", Duration::from_secs(20)).await;
    let mut agent = connect_agent(port, &task_token).await;

    // Write an artifact that leaks the secret value, and try to register it.
    let leaky = PathBuf::from(&worktree_path).join("plan.html");
    std::fs::write(&leaky, format!("<html><body><pre>token = {secret_value}</pre></body></html>")).unwrap();
    let reg = request(&mut agent, &Envelope::message("reg", "artifact.register", serde_json::json!({ "path": leaky.to_string_lossy(), "kind": "plan" })), &mut sink).await;
    assert_eq!(reg.msg_type, "error", "a secret-bearing artifact must be refused: {reg:?}");
    assert_eq!(reg.payload["code"], "forbidden");

    // A clean artifact registers fine (the negative control).
    let clean = PathBuf::from(&worktree_path).join("clean.html");
    std::fs::write(&clean, "<html><body><p>no secrets here</p></body></html>").unwrap();
    let ok = request(&mut agent, &Envelope::message("reg2", "artifact.register", serde_json::json!({ "path": clean.to_string_lossy(), "kind": "plan" })), &mut sink).await;
    assert_eq!(ok.msg_type, "artifact.register", "a clean artifact registers: {ok:?}");

    let _ = request(&mut root, &Envelope::message("cancel", "dispatch.cancel", serde_json::json!({ "card_id": card_id })), &mut sink).await;
}
