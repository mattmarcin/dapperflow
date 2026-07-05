//! Configured agents end to end (Phase 1.5): agents.add/list, launcher resolution
//! precedence in dispatch, extra-env reaching the spawned process (the cc-alt case),
//! the extra-args caution surface, and the remove-while-live guard.
//!
//! A launcher is made to run a `cmd.exe /k` stub through the `DFLOW_LAUNCH_<NAME>`
//! override (keyed by launcher name), so no real agent CLI is needed. The stub echoes
//! `%CC_ALT_ENV%`, so its presence in the output proves the launcher's extra env was
//! merged into the spawn environment.

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use dflow_proto::{
    AgentRemoved, AgentResult, AgentsDetected, AgentsListResult, CardCreated, CardGetResult,
    DispatchStarted, Envelope, ProjectAdded,
};

/// The launcher named `ccalt` runs this stub: echo the config-dir env var, then cwd.
const CCALT_LAUNCH: &str = r#"["cmd.exe","/d","/k","echo CCENV=[%CC_ALT_ENV%] && cd"]"#;

async fn add_card(ws: &mut Ws, sink: &mut Vec<u8>, project_id: &str, title: &str) -> String {
    let resp = request(
        ws,
        &Envelope::message(
            "card",
            "card.create",
            serde_json::json!({ "title": title, "type": "feature", "project_id": project_id }),
        ),
        sink,
    )
    .await;
    let created: CardCreated = resp.decode_payload().unwrap();
    created.card_id
}

#[tokio::test]
async fn configured_launchers_resolve_and_carry_env() {
    let data_dir = unique_data_dir("agents");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_CCALT", CCALT_LAUNCH)]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // ---- project + cards. ----
    let resp = request(
        &mut ws,
        &Envelope::message("p1", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project: ProjectAdded = resp.decode_payload().unwrap();
    let card_a = add_card(&mut ws, &mut sink, &project.project_id, "Ship the login flow").await;
    let card_b = add_card(&mut ws, &mut sink, &project.project_id, "Second dispatch").await;

    // ---- agents.detect runs the real PATH scan; response must decode. ----
    let resp = request(
        &mut ws,
        &Envelope::message("det", "agents.detect", serde_json::json!({})),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "agents.detect", "detect failed: {resp:?}");
    let _detected: AgentsDetected = resp.decode_payload().unwrap();

    // ---- agents.add: cc-alt style launcher (claude family + second config dir). ----
    let resp = request(
        &mut ws,
        &Envelope::message(
            "a1",
            "agents.add",
            serde_json::json!({
                "name": "ccalt",
                "adapter": "claude",
                "command": "claude",
                "extra_env": { "CC_ALT_ENV": "second-subscription" },
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "agents.add", "agents.add failed: {resp:?}");
    let added: AgentResult = resp.decode_payload().unwrap();
    assert_eq!(added.agent.adapter, "claude");
    assert_eq!(added.agent.source, "custom");
    assert!(added.agent.enabled);
    assert!(!added.agent.caution, "no dangerous args on ccalt");

    // A second launcher whose default args weaken safety -> caution in the list.
    let resp = request(
        &mut ws,
        &Envelope::message(
            "a2",
            "agents.add",
            serde_json::json!({
                "name": "yolo",
                "adapter": "claude",
                "command": "claude",
                "extra_args": ["--dangerously-skip-permissions"],
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "agents.add", "second agents.add failed: {resp:?}");

    // ---- agents.list reflects both, with the caution flag computed. ----
    let resp = request(
        &mut ws,
        &Envelope::message("l1", "agents.list", serde_json::json!({})),
        &mut sink,
    )
    .await;
    let listing: AgentsListResult = resp.decode_payload().unwrap();
    let ccalt = listing.agents.iter().find(|a| a.name == "ccalt").expect("ccalt listed");
    assert!(!ccalt.caution);
    assert_eq!(ccalt.extra_env.get("CC_ALT_ENV").map(String::as_str), Some("second-subscription"));
    let yolo = listing.agents.iter().find(|a| a.name == "yolo").expect("yolo listed");
    assert!(yolo.caution, "dangerous extra arg must set caution");

    // Adding a duplicate name is a structured bad_request.
    let resp = request(
        &mut ws,
        &Envelope::message(
            "a3",
            "agents.add",
            serde_json::json!({ "name": "ccalt", "adapter": "claude", "command": "claude" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "error");
    assert_eq!(resp.payload["code"], "bad_request");

    // An unknown adapter is rejected.
    let resp = request(
        &mut ws,
        &Envelope::message(
            "a4",
            "agents.add",
            serde_json::json!({ "name": "weird", "adapter": "gemini", "command": "gemini" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(resp.payload["code"], "bad_request", "unknown adapter must be refused: {resp:?}");

    // ---- dispatch via the explicit `agent` param: launcher resolves, env reaches
    //      the process, the session runs under the claude adapter. ----
    let started = dispatch_and_check_env(&mut ws, &mut sink, serde_json::json!({
        "card_id": card_a, "agent": "ccalt"
    }))
    .await;
    assert_eq!(started.harness, "claude", "session runs under the adapter family");
    assert_eq!(started.agent.as_deref(), Some("ccalt"), "response names the launcher");

    // ---- dispatch via the `harness` param naming the launcher: same resolution
    //      (harness > built-in when a same-named enabled launcher exists). ----
    let started_b = dispatch_and_check_env(&mut ws, &mut sink, serde_json::json!({
        "card_id": card_b, "harness": "ccalt"
    }))
    .await;
    assert_eq!(started_b.agent.as_deref(), Some("ccalt"), "harness param resolved the launcher");

    // ---- the dispatched event records the launcher name. ----
    let resp = request(
        &mut ws,
        &Envelope::message("g1", "card.get", serde_json::json!({ "card_id": card_a })),
        &mut sink,
    )
    .await;
    let got: CardGetResult = resp.decode_payload().unwrap();
    let dispatched = got
        .events
        .iter()
        .find(|e| e.kind == "dispatched")
        .expect("dispatched event present");
    assert_eq!(dispatched.payload["agent"], "ccalt");
    assert_eq!(got.sessions[0].harness, "claude");

    // ---- agents.remove refuses while a session is live; disable is the way out. ----
    let resp = request(
        &mut ws,
        &Envelope::message("r1", "agents.remove", serde_json::json!({ "id": "ccalt" })),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "error", "remove must be refused while live: {resp:?}");
    assert_eq!(resp.payload["code"], "bad_request");
    assert!(
        resp.payload["message"].as_str().unwrap_or("").contains("disable it instead"),
        "error should suggest disabling: {resp:?}"
    );

    // Disabling instead is allowed even while live.
    let resp = request(
        &mut ws,
        &Envelope::message("u1", "agents.update", serde_json::json!({ "id": "ccalt", "enabled": false })),
        &mut sink,
    )
    .await;
    let updated: AgentResult = resp.decode_payload().unwrap();
    assert!(!updated.agent.enabled, "ccalt disabled");

    // ---- cancel both dispatches, then removal succeeds. ----
    for (n, card) in [("x1", &card_a), ("x2", &card_b)] {
        let resp = request(
            &mut ws,
            &Envelope::message(n, "dispatch.cancel", serde_json::json!({ "card_id": card })),
            &mut sink,
        )
        .await;
        assert_eq!(resp.msg_type, "dispatch.cancel", "cancel failed: {resp:?}");
    }

    let resp = request(
        &mut ws,
        &Envelope::message("r2", "agents.remove", serde_json::json!({ "id": "ccalt" })),
        &mut sink,
    )
    .await;
    assert_eq!(resp.msg_type, "agents.remove", "remove after cancel failed: {resp:?}");
    let removed: AgentRemoved = resp.decode_payload().unwrap();
    assert_eq!(removed.removed, "ccalt");

    // ---- clean shutdown. ----
    let _ = request(
        &mut ws,
        &Envelope::message("s1", "daemon.shutdown", serde_json::json!({})),
        &mut sink,
    )
    .await;
}

/// Dispatch, attach, and assert the launcher's extra env (`CC_ALT_ENV`) and the
/// worktree cwd both appear in the stub's output. Returns the dispatch response.
async fn dispatch_and_check_env(
    ws: &mut Ws,
    sink: &mut Vec<u8>,
    dispatch_payload: serde_json::Value,
) -> DispatchStarted {
    let resp = request(ws, &Envelope::message("d", "dispatch.start", dispatch_payload), sink).await;
    assert_eq!(resp.msg_type, "dispatch.start", "dispatch failed: {resp:?}");
    let started: DispatchStarted = resp.decode_payload().unwrap();
    let worktree_path = started.worktree_path.clone();
    assert!(Path::new(&worktree_path).is_dir(), "worktree missing: {worktree_path}");

    let resp = request(
        ws,
        &Envelope::message(
            "at",
            "session.attach",
            serde_json::json!({ "session_id": started.session_id, "cols": 120, "rows": 32 }),
        ),
        sink,
    )
    .await;
    assert_eq!(resp.msg_type, "session.attach", "attach failed: {resp:?}");

    let mut seen = String::from_utf8_lossy(sink).to_string();
    if !seen.contains("CCENV=[second-subscription]") {
        seen.push_str(&collect_output_until(ws, "CCENV=[second-subscription]", Duration::from_secs(15)).await);
    }
    assert!(
        seen.contains("CCENV=[second-subscription]"),
        "launcher extra env did not reach the spawned process; output: {seen:?}"
    );
    // Detach so the next dispatch's attach starts from a clean sink.
    let _ = request(
        ws,
        &Envelope::message(
            "dt",
            "session.detach",
            serde_json::json!({ "session_id": started.session_id }),
        ),
        sink,
    )
    .await;
    sink.clear();
    started
}
