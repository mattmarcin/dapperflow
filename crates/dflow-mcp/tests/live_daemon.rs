//! Integration tests against a real, workspace-built `dflowd` binary.
//!
//! Every test boots its own daemon on an isolated temp `DFLOW_DATA_DIR`
//! (mirroring `crates/dflowd/tests/common/mod.rs`), pins the server to that
//! endpoint via `DflowMcp::with_endpoint`, and drives the public tool methods.
//! One test additionally speaks real MCP JSON-RPC to the shipped binary over
//! stdio with `DFLOW_DATA_DIR` set, proving the production discovery path.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dflow_mcp::daemon::Daemon;
use dflow_mcp::runtime::DaemonEndpoint;
use dflow_mcp::server::{
    BoardQueryParams, CardCreateParams, CardMoveParams, CardUpdateParams, DflowMcp,
    DispatchStartParams, KnowledgeDigestParams, SessionPeekParams, SteerSessionParams,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;

// ---- daemon harness ----

struct DaemonGuard(Child);

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Build (cached no-op when fresh) and locate the workspace `dflowd` binary.
fn dflowd_bin() -> PathBuf {
    static BIN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "dflowd", "--quiet"])
            .current_dir(workspace_root())
            .status()
            .expect("run cargo build -p dflowd");
        assert!(status.success(), "cargo build -p dflowd failed");
        let exe = if cfg!(windows) { "dflowd.exe" } else { "dflowd" };
        let path = workspace_root().join("target").join("debug").join(exe);
        assert!(path.exists(), "built dflowd not found at {}", path.display());
        path
    })
    .clone()
}

fn unique_data_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-mcp-it-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Start dflowd against `data_dir`; return the guard and the endpoint.
fn start_daemon(data_dir: &Path) -> (DaemonGuard, DaemonEndpoint) {
    let mut cmd = Command::new(dflowd_bin());
    cmd.env("DFLOW_DATA_DIR", data_dir).env("DFLOW_LOG", "warn");
    if let Ok(log) = std::fs::File::create(data_dir.join("daemon.log")) {
        if let Ok(log2) = log.try_clone() {
            cmd.stdout(Stdio::from(log));
            cmd.stderr(Stdio::from(log2));
        }
    }
    let child = cmd.spawn().expect("spawn dflowd");
    let guard = DaemonGuard(child);

    let dir = dflow_core::DataDir::at(data_dir);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match dflow_mcp::runtime::discover_at(&dir) {
            Ok(ep) => return (guard, ep),
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => panic!("daemon never published a usable runtime file: {e}"),
        }
    }
}

/// A tiny real git repo (project.add validates git + default branch).
fn make_git_repo(tag: &str) -> PathBuf {
    let dir = unique_data_dir(&format!("repo-{tag}"));
    let run = |args: &[&str]| {
        let out = Command::new("git").args(args).current_dir(&dir).output().expect("git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.email", "test@dapperflow.local"]);
    run(&["config", "user.name", "DapperFlow Test"]);
    std::fs::write(dir.join("README.md"), "# test\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-m", "init"]);
    dir
}

// ---- helpers over the tool surface ----

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_error(result: &CallToolResult) -> bool {
    result.is_error.unwrap_or(false)
}

/// Extract the first `[kind:id]` token's id from rendered text.
fn extract_id(text: &str, kind: &str) -> String {
    let tag = format!("[{kind}:");
    let start = text.find(&tag).unwrap_or_else(|| panic!("no {tag} token in: {text}")) + tag.len();
    let end = text[start..].find(']').expect("closing bracket") + start;
    text[start..end].to_string()
}

/// Graceful daemon shutdown so spawned shells never outlive the test.
fn shutdown(ep: &DaemonEndpoint) {
    if let Ok(mut d) = Daemon::connect_to(&ep.endpoint, &ep.token) {
        let _ = d.request_value("daemon.shutdown", serde_json::json!({}));
    }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap()
}

// ---- tests ----

/// Deliverable 6: each board tool round-trips against real daemon state.
#[test]
fn card_lifecycle_round_trips_through_the_tools() {
    let data_dir = unique_data_dir("cards");
    let (_guard, ep) = start_daemon(&data_dir);
    let repo = make_git_repo("cards");

    // Register a project over the wire (project registration is app UI work,
    // not a Concertmaster tool).
    let mut wire = Daemon::connect_to(&ep.endpoint, &ep.token).unwrap();
    let added: dflow_proto::ProjectAdded = wire
        .request("project.add", dflow_proto::ProjectAdd { path: repo.display().to_string() })
        .unwrap();

    let srv = DflowMcp::with_endpoint(ep.clone());
    let rt = rt();

    // project_list sees it, with the stable [project:id] token.
    let out = rt.block_on(srv.project_list()).unwrap();
    let text = text_of(&out);
    assert!(!is_error(&out), "{text}");
    assert!(text.contains(&format!("[project:{}]", added.project_id)), "{text}");

    // card_create -> [card:id].
    let out = rt
        .block_on(srv.card_create(Parameters(CardCreateParams {
            title: "MCP round trip card".into(),
            card_type: Some("bug".into()),
            project_id: Some(added.project_id.clone()),
            brief: Some("created by the dflow-mcp integration test".into()),
            lane: None,
            priority: Some(2),
        })))
        .unwrap();
    let text = text_of(&out);
    assert!(!is_error(&out), "{text}");
    assert!(text.contains("lane=inbox") && text.contains("type=bug"), "{text}");
    let card_id = extract_id(&text, "card");

    // board_query filters find it.
    let out = rt
        .block_on(srv.board_query(Parameters(BoardQueryParams {
            project_id: Some(added.project_id.clone()),
            lane: Some("inbox".into()),
            card_type: Some("bug".into()),
            limit: None,
        })))
        .unwrap();
    let text = text_of(&out);
    assert!(text.contains(&card_id) && text.contains("MCP round trip card"), "{text}");

    // card_update changes the title and priority.
    let out = rt
        .block_on(srv.card_update(Parameters(CardUpdateParams {
            card_id: card_id.clone(),
            title: Some("MCP round trip card v2".into()),
            brief: None,
            card_type: None,
            priority: Some(5),
        })))
        .unwrap();
    let text = text_of(&out);
    assert!(text.contains("v2") && text.contains("prio=5"), "{text}");

    // card_move relocates it; the daemon confirms the new lane.
    let out = rt
        .block_on(srv.card_move(Parameters(CardMoveParams {
            card_id: card_id.clone(),
            column: "shaping".into(),
        })))
        .unwrap();
    let text = text_of(&out);
    assert!(text.contains("lane=shaping"), "{text}");

    // The daemon state agrees (verified over the wire, not via the tool).
    let got: dflow_proto::CardGetResult = wire
        .request(
            "card.get",
            dflow_proto::CardGet { card_id: card_id.clone(), events_limit: None, events_before: None },
        )
        .unwrap();
    assert_eq!(got.card.lane, "shaping");
    assert_eq!(got.card.title, "MCP round trip card v2");

    shutdown(&ep);
}

/// Deliverables 2 and 6: dispatch_start through a configured launcher leases a
/// real worktree and spawns a real (shell) session; fleet_status and
/// session_peek then observe it.
#[test]
fn dispatch_fleet_and_peek_round_trip() {
    let data_dir = unique_data_dir("dispatch");
    let (_guard, ep) = start_daemon(&data_dir);
    let repo = make_git_repo("dispatch");

    let mut wire = Daemon::connect_to(&ep.endpoint, &ep.token).unwrap();
    let added: dflow_proto::ProjectAdded = wire
        .request("project.add", dflow_proto::ProjectAdd { path: repo.display().to_string() })
        .unwrap();
    // A custom launcher so dispatch needs no real agent CLI (and no tokens).
    let _agent: dflow_proto::AgentResult = wire
        .request(
            "agents.add",
            dflow_proto::AgentAdd {
                name: "psh".into(),
                adapter: "custom".into(),
                command: "powershell".into(),
                extra_args: vec![],
                extra_env: Default::default(),
            },
        )
        .unwrap();

    let srv = DflowMcp::with_endpoint(ep.clone());
    let rt = rt();

    let out = rt
        .block_on(srv.card_create(Parameters(CardCreateParams {
            title: "dispatch me".into(),
            card_type: None,
            project_id: Some(added.project_id.clone()),
            brief: Some("say hello and wait".into()),
            lane: Some("ready".into()),
            priority: None,
        })))
        .unwrap();
    let card_id = extract_id(&text_of(&out), "card");

    let out = rt
        .block_on(srv.dispatch_start(Parameters(DispatchStartParams {
            card_id: card_id.clone(),
            recipe: None,
            agent: Some("psh".into()),
            harness: None,
            model: None,
            effort: None,
        })))
        .unwrap();
    let text = text_of(&out);
    assert!(!is_error(&out), "{text}");
    assert!(text.contains(&format!("[card:{card_id}]")), "{text}");
    let session_id = extract_id(&text, "session");
    assert!(text.contains("worktree="), "{text}");

    // fleet_status sees the session with its card linkage.
    let out = rt.block_on(srv.fleet_status()).unwrap();
    let text = text_of(&out);
    assert!(text.contains(&format!("[session:{session_id}]")), "{text}");
    assert!(text.contains(&format!("[card:{card_id}]")), "{text}");

    // session_peek returns plain text from the live screen, eventually
    // (PowerShell takes a moment to paint its banner/prompt).
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut screen = String::new();
    while Instant::now() < deadline {
        let out = rt
            .block_on(srv.session_peek(Parameters(SessionPeekParams {
                session_id: session_id.clone(),
                lines: Some(10),
            })))
            .unwrap();
        let text = text_of(&out);
        assert!(!is_error(&out), "{text}");
        if !text.contains("screen is currently blank") {
            screen = text;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(screen.contains(&format!("[session:{session_id}] screen")), "{screen}");
    let body = screen.split_once('\n').map(|(_, b)| b).unwrap_or("");
    assert!(!body.trim().is_empty(), "peek body should carry screen text: {screen}");
    assert!(body.lines().count() <= 10, "line bound respected: {screen}");

    shutdown(&ep);
}

/// Deliverable 4: the guarded steering contract against real sessions.
#[test]
fn steering_guardrails_hold_against_a_real_daemon() {
    let data_dir = unique_data_dir("steer");
    let (_guard, ep) = start_daemon(&data_dir);

    let mut wire = Daemon::connect_to(&ep.endpoint, &ep.token).unwrap();
    // Real shell processes wearing adapter-family labels: explicit command wins,
    // the harness string is recorded as the family (Phase 0 session.create).
    let mut create = |harness: &str| -> String {
        let created: dflow_proto::SessionCreated = wire
            .request(
                "session.create",
                dflow_proto::SessionCreate {
                    card_id: None,
                    worktree_id: None,
                    harness: harness.into(),
                    agent: None,
                    command: Some(vec!["powershell".into()]),
                    cols: 100,
                    rows: 30,
                    cwd: None,
                    env: Default::default(),
                    first_prompt: None,
                },
            )
            .unwrap();
        created.session_id
    };
    let cursor_session = create("cursor");
    let unknown_session = create("powershell");
    let claude_session = create("claude");

    let srv = DflowMcp::with_endpoint(ep.clone());
    let rt = rt();
    let steer = |sid: &str| {
        rt.block_on(srv.steer_session(Parameters(SteerSessionParams {
            session_id: sid.into(),
            text: "continue with the plan".into(),
        })))
        .unwrap()
    };

    // no_auto_steer manifest (cursor) refuses, absolutely, before any wire send.
    let out = steer(&cursor_session);
    let text = text_of(&out);
    assert!(text.contains("no_auto_steer"), "{text}");

    // Unknown adapter families refuse by default.
    let out = steer(&unknown_session);
    let text = text_of(&out);
    assert!(text.contains("not a known adapter family"), "{text}");

    // A steerable family passes the guards and reaches the wire, where this daemon
    // build now routes session.send_verified (phase8 vault + M4 wiring). The tool no
    // longer reports a daemon gap; it returns a real one-shot steering outcome - either
    // a submitted attribution, or an escalate-on-failed-submit (the exact outcome
    // depends on whether the underlying shell's composer accepts the text).
    let out = steer(&claude_session);
    let text = text_of(&out);
    assert!(!text.contains("does not route"), "the verb is routed now: {text}");
    assert!(
        text.contains("One-shot") || text.contains("escalate to the human"),
        "expected a real steering outcome once the verb is routed: {text}"
    );

    // The verb now reaches the session, so each steer consumes a budget slot (it is only
    // refunded when nothing reaches the session). Once the per-session rolling-hour
    // budget is spent, further steers report the rate limit - the guardrail engaging.
    let mut hit_budget = false;
    for _ in 0..5 {
        let out = steer(&claude_session);
        if text_of(&out).contains("budget") {
            hit_budget = true;
            break;
        }
    }
    assert!(hit_budget, "the per-session steer budget must engage now that the verb is routed");

    // A session id the daemon does not know.
    let out = steer("01UNKNOWNSESSIONID0000000000");
    let text = text_of(&out);
    assert!(text.contains("no session with id"), "{text}");

    shutdown(&ep);
}

/// Deliverable 6: knowledge tools degrade with an honest, structured message
/// while the daemon routes know.* only for agent-scoped tokens.
#[test]
fn knowledge_tools_degrade_gracefully_on_desktop_scope() {
    let data_dir = unique_data_dir("know");
    let (_guard, ep) = start_daemon(&data_dir);
    let repo = make_git_repo("know");

    let mut wire = Daemon::connect_to(&ep.endpoint, &ep.token).unwrap();
    let added: dflow_proto::ProjectAdded = wire
        .request("project.add", dflow_proto::ProjectAdd { path: repo.display().to_string() })
        .unwrap();

    let srv = DflowMcp::with_endpoint(ep.clone());
    let rt = rt();
    let out = rt
        .block_on(srv.knowledge_digest(Parameters(KnowledgeDigestParams {
            project_id: Some(added.project_id),
        })))
        .unwrap();
    let text = text_of(&out);
    // Merge-proof assertion: either the daemon routes know.index for this scope
    // (post-merge) and we render a digest, or it does not and the tool says so.
    assert!(
        text.contains("digest") || text.contains("know.index"),
        "unexpected knowledge_digest output: {text}"
    );

    shutdown(&ep);
}

/// Deliverable 6: the shipped binary boots over stdio against a real daemon
/// via DFLOW_DATA_DIR discovery, serves the tool list, and executes a tool.
#[test]
fn stdio_binary_serves_mcp_against_real_daemon() {
    let data_dir = unique_data_dir("stdio");
    let (_guard, ep) = start_daemon(&data_dir);

    let mut child = Command::new(env!("CARGO_BIN_EXE_dflow-mcp"))
        .arg("serve")
        .env("DFLOW_DATA_DIR", &data_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dflow-mcp serve");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    // Reader thread so a wedged server fails the test instead of hanging it.
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut send = |v: serde_json::Value| {
        stdin
            .write_all((serde_json::to_string(&v).unwrap() + "\n").as_bytes())
            .expect("write to server stdin");
    };
    let recv = |rx: &mpsc::Receiver<String>| -> serde_json::Value {
        let line = rx.recv_timeout(Duration::from_secs(30)).expect("server response");
        serde_json::from_str(&line).expect("valid JSON-RPC line")
    };

    send(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "itest", "version": "0" }
        }
    }));
    let init = recv(&rx);
    assert_eq!(init["id"], 1, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "dflow-mcp", "{init}");
    assert!(
        init["result"]["instructions"].as_str().unwrap_or("").contains("human-only"),
        "{init}"
    );

    send(serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));

    send(serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
    let tools = recv(&rx);
    let names: Vec<String> = tools["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names.len(), 13, "{names:?}");
    for expected in ["fleet_status", "steer_session", "card_create", "session_peek"] {
        assert!(names.iter().any(|n| n == expected), "missing {expected}: {names:?}");
    }
    for forbidden in ["merge", "push", "discard", "recipe", "env", "kill"] {
        assert!(
            !names.iter().any(|n| n.contains(forbidden)),
            "excluded capability leaked: {forbidden} in {names:?}"
        );
    }

    send(serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": { "name": "fleet_status", "arguments": {} }
    }));
    let call = recv(&rx);
    let text = call["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("sessions:"), "fleet_status over stdio: {call}");

    drop(stdin); // EOF ends the stdio transport; the server exits.
    let status = wait_with_timeout(&mut child, Duration::from_secs(10));
    assert!(status.is_some(), "dflow-mcp did not exit after stdin EOF");
    shutdown(&ep);
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
