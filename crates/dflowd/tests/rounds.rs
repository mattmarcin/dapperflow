//! M4 Concertmaster rounds end-to-end (`product.md` / Concertmaster rounds).
//!
//! Drives the real daemon: `round.start` dispatches a headless round session with a
//! round-scoped token echoed into the stub's scrollback (exactly as dispatch injects it),
//! the test reads that real round token back and files the round's digest over it, and
//! asserts the escalation-only contract - at most ONE Needs You digest, deduped on re-run,
//! `round_completed` on the round card, and the round scope's capability boundary. A
//! second test drives the scheduler tick.

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The round stub echoes its injected round token + round card into the PTY, then stays
/// alive so the test can attach and read the token back (the `harness: "stub"` launcher
/// is resolved via the `DFLOW_LAUNCH_STUB` test seam).
const ROUND_STUB: &str =
    r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%; DROUND=%DFLOW_ROUND%; DCARD=%DFLOW_CARD%"]"#;

/// Connect and authenticate with an explicit token, asserting the granted scope label.
async fn connect_scope(port: u16, token: &str, client: ClientKind, expect: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let hello = Envelope::message(
        "auth",
        "auth.hello",
        AuthHello { token: token.to_string(), client, proto_versions: vec![PROTOCOL_VERSION] },
    );
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let welcome = next_envelope(&mut ws).await;
    assert_eq!(welcome.msg_type, "auth.welcome", "auth failed: {welcome:?}");
    assert_eq!(welcome.payload["scope"], expect, "unexpected scope: {welcome:?}");
    ws
}

/// Pull the round token out of the stub scrollback (`DTOKEN=<48 alnum>`).
fn extract_token(screen: &str) -> Option<String> {
    let start = screen.find("DTOKEN=")? + "DTOKEN=".len();
    let token: String = screen[start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (token.len() >= 40).then_some(token)
}

#[tokio::test]
async fn round_start_spawns_scoped_session_and_digest_dedupes() {
    let data_dir = unique_data_dir("rounds");
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", ROUND_STUB)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Start a global floor-check round.
    let started = request(
        &mut root,
        &Envelope::message("r1", "round.start", serde_json::json!({ "round_type": "floor_check", "harness": "stub" })),
        &mut sink,
    )
    .await;
    assert_eq!(started.msg_type, "round.start", "round.start failed: {started:?}");
    let round_card = started.payload["round_card"].as_str().unwrap().to_string();
    let session_id = started.payload["session_id"].as_str().unwrap().to_string();
    assert_eq!(started.payload["scope"], "all");
    assert_eq!(started.payload["round_type"], "floor_check");

    // Read the injected round token + round card back from the stub scrollback.
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
    let round_token = extract_token(&screen)
        .unwrap_or_else(|| panic!("round token not in scrollback: {screen:?}"));
    assert!(screen.contains(&format!("DROUND={round_card}")), "DFLOW_ROUND not injected: {screen:?}");

    // The round token authenticates as the reduced "round" scope.
    let mut roundc = connect_scope(port, &round_token, ClientKind::Desktop, "round").await;

    // First digest: filed, not deduped.
    let d1 = request(
        &mut roundc,
        &Envelope::message("d1", "round.digest", serde_json::json!({ "body": "drift: card X silent 3d", "findings": 3 })),
        &mut sink,
    )
    .await;
    assert_eq!(d1.msg_type, "round.digest", "round.digest failed: {d1:?}");
    assert_eq!(d1.payload["round_card"], round_card);
    assert_eq!(d1.payload["findings"], 3);
    assert_eq!(d1.payload["deduped"], false);

    // Second digest: dedupes onto the same item (the at-most-one contract).
    let d2 = request(
        &mut roundc,
        &Envelope::message("d2", "round.digest", serde_json::json!({ "body": "second look", "findings": 5 })),
        &mut sink,
    )
    .await;
    assert_eq!(d2.payload["deduped"], true, "second digest must dedupe: {d2:?}");

    // Exactly ONE Needs You digest item for the round card.
    let nyou = request(&mut root, &Envelope::message("n", "needs_you.list", serde_json::json!({})), &mut sink).await;
    let digests: Vec<_> = nyou.payload["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["card_id"] == round_card.as_str() && i["kind"] == "round_digest")
        .collect();
    assert_eq!(digests.len(), 1, "at most one digest per round: {:?}", nyou.payload["items"]);

    // The round card timeline carries round_started + round_completed.
    let cget = request(&mut root, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": round_card })), &mut sink).await;
    let kinds: Vec<String> = cget.payload["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(kinds.iter().any(|k| k == "round_started"), "round_started missing: {kinds:?}");
    assert!(kinds.iter().any(|k| k == "round_completed"), "round_completed missing: {kinds:?}");

    // Capability boundary: a round token cannot dispatch or touch the vault.
    let forbidden = request(
        &mut roundc,
        &Envelope::message("f1", "dispatch.start", serde_json::json!({ "card_id": round_card })),
        &mut sink,
    )
    .await;
    assert_eq!(forbidden.msg_type, "error");
    assert_eq!(forbidden.payload["code"], "forbidden", "round must not dispatch: {forbidden:?}");

    let vault = request(
        &mut roundc,
        &Envelope::message("f2", "env.set", serde_json::json!({ "project_id": "x", "key": "K", "value": "V", "kind": "secret" })),
        &mut sink,
    )
    .await;
    assert_eq!(vault.payload["code"], "forbidden", "round must not touch the vault: {vault:?}");
}

#[tokio::test]
async fn round_start_dedupes_the_round_card_on_rerun() {
    let data_dir = unique_data_dir("rounds-rerun");
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", ROUND_STUB)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let first = request(
        &mut root,
        &Envelope::message("r1", "round.start", serde_json::json!({ "round_type": "floor_check", "harness": "stub" })),
        &mut sink,
    )
    .await;
    let card_a = first.payload["round_card"].as_str().unwrap().to_string();

    // Re-running the same type + scope reuses the same round card (dedupe on re-run).
    let second = request(
        &mut root,
        &Envelope::message("r2", "round.start", serde_json::json!({ "round_type": "floor_check", "harness": "stub" })),
        &mut sink,
    )
    .await;
    let card_b = second.payload["round_card"].as_str().unwrap().to_string();
    assert_eq!(card_a, card_b, "a re-run must reuse the round card, not fork a new one");
}

#[tokio::test]
async fn scheduler_dispatches_a_due_round() {
    let data_dir = unique_data_dir("roundsched");
    let repo = scratch_repo(&data_dir);
    // A fast tick so the test does not wait a minute for the scheduler.
    let (_daemon, port, token) =
        start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", ROUND_STUB), ("DFLOW_ROUND_TICK_MS", "400")]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Register the project and enable a gardener schedule due immediately (interval 0).
    let padd = request(
        &mut root,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let upd = request(
        &mut root,
        &Envelope::message(
            "u",
            "project.update",
            serde_json::json!({ "project_id": project_id, "gardener_schedule": "{\"enabled\":true,\"interval_minutes\":0}" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(upd.msg_type, "project.update", "schedule update failed: {upd:?}");

    // Poll for the scheduler to dispatch a garden round card for the project.
    let mut round_card = None;
    for _ in 0..25 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        let q = request(
            &mut root,
            &Envelope::message("q", "card.query", serde_json::json!({ "filter": { "project_id": project_id } })),
            &mut sink,
        )
        .await;
        if let Some(card) = q.payload["cards"].as_array().unwrap().iter().find(|c| {
            c["title"].as_str().unwrap_or("").starts_with("Round: garden")
        }) {
            round_card = Some(card["id"].as_str().unwrap().to_string());
            break;
        }
    }
    let round_card = round_card.expect("scheduler never dispatched a garden round");

    // The scheduled round emitted the same contract: a round_started event on its card.
    let cget = request(&mut root, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": round_card })), &mut sink).await;
    let kinds: Vec<String> = cget.payload["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(kinds.iter().any(|k| k == "round_started"), "scheduled round_started missing: {kinds:?}");
}
