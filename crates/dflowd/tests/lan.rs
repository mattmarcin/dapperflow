//! M6 opt-in LAN listener end-to-end (`security.md` / Remote access trust model).
//!
//! Drives the real daemon: enable the second listener, fetch the PWA at `/m`, mint a QR
//! pairing, complete a phone-scoped WS handshake, prove the capability boundary (allowed
//! verbs pass, forbidden verbs are rejected, the root token is refused over the LAN), and
//! prove per-device revocation kills the token.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Write a fixture PWA dist dir the daemon will serve at `/m` (via `DFLOW_PWA_DIST`).
fn fixture_pwa_dir(base: &std::path::Path) -> std::path::PathBuf {
    let dir = base.join("pwa");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html><title>m</title>PWA-FIXTURE-OK").unwrap();
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("assets").join("app.js"), "console.log('fixture')").unwrap();
    dir
}

/// A minimal blocking HTTP GET over loopback; returns `(status_code, body)`.
fn http_get(port: u16, path: &str) -> (u16, String) {
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect LAN http");
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    let _ = stream.read_to_string(&mut resp);
    let status = resp
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// Connect to a LAN `/ws` and send `auth.hello`; return the socket and the reply.
async fn lan_hello(port: u16, token: &str, client: ClientKind) -> (Ws, Envelope) {
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect LAN ws");
    let hello = Envelope::message(
        "auth",
        "auth.hello",
        AuthHello { token: token.to_string(), client, proto_versions: vec![PROTOCOL_VERSION] },
    );
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let reply = next_envelope_or_close(&mut ws).await;
    (ws, reply)
}

/// The next text envelope, or a synthetic `{"type":"__closed__"}` when the socket closes
/// first (an auth rejection closes the socket with a distinct code).
async fn next_envelope_or_close(ws: &mut Ws) -> Envelope {
    loop {
        match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                return serde_json::from_str(t.as_str()).expect("parse envelope");
            }
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_))) | Ok(None) => {
                return Envelope::message("x", "__closed__", serde_json::json!({}));
            }
            Ok(Some(Ok(_))) => continue,
            Err(_) => return Envelope::message("x", "__timeout__", serde_json::json!({})),
        }
    }
}

#[tokio::test]
async fn lan_listener_serves_pwa_pairs_and_enforces_phone_scope() {
    let data_dir = unique_data_dir("lan");
    let pwa = fixture_pwa_dir(&data_dir);
    let (_daemon, port, token) =
        start_daemon(&data_dir, &[("DFLOW_PWA_DIST", &pwa.to_string_lossy())]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Enable the LAN listener on an OS-assigned port (0), so tests never collide.
    let enabled = request(
        &mut root,
        &Envelope::message("e", "daemon.lan.enable", serde_json::json!({ "port": 0 })),
        &mut sink,
    )
    .await;
    assert_eq!(enabled.msg_type, "daemon.lan.enable", "enable failed: {enabled:?}");
    assert_eq!(enabled.payload["enabled"], true);
    assert_eq!(enabled.payload["bound"], true);
    let lan_port = enabled.payload["port"].as_u64().expect("bound LAN port") as u16;
    assert!(lan_port > 0, "expected a bound LAN port: {enabled:?}");
    assert!(
        enabled.payload["caveat"].as_str().unwrap_or("").contains("without TLS"),
        "status must carry the honest no-TLS caveat: {enabled:?}"
    );

    // The PWA is served at /m from the fixture dist.
    let (status, body) = http_get(lan_port, "/m");
    assert_eq!(status, 200, "GET /m should be 200");
    assert!(body.contains("PWA-FIXTURE-OK"), "GET /m should serve the PWA index: {body:?}");
    // And an asset under /m/.
    let (astatus, abody) = http_get(lan_port, "/m/assets/app.js");
    assert_eq!(astatus, 200);
    assert!(abody.contains("fixture"), "GET /m/assets/app.js should serve the asset");
    // /health is up on the LAN listener too.
    assert_eq!(http_get(lan_port, "/health").0, 200);

    // Mint a QR pairing (loopback/owner scope). The payload shape matches mobile.md.
    let pairing = request(
        &mut root,
        &Envelope::message("p", "daemon.lan.pair", serde_json::json!({ "name": "Matt phone" })),
        &mut sink,
    )
    .await;
    assert_eq!(pairing.msg_type, "daemon.lan.pair", "pair failed: {pairing:?}");
    let token_id = pairing.payload["token_id"].as_str().unwrap().to_string();
    let phone_token = pairing.payload["payload"]["token"].as_str().unwrap().to_string();
    let ws_url = pairing.payload["payload"]["url"].as_str().unwrap();
    let pair_url = pairing.payload["pair_url"].as_str().unwrap();
    assert!(ws_url.starts_with("ws://") && ws_url.ends_with("/ws"), "ws url: {ws_url}");
    assert!(pair_url.starts_with("http://") && pair_url.contains("/m#pair="), "pair url: {pair_url}");
    assert_eq!(pairing.payload["payload"]["name"], "Matt phone");
    assert!(!phone_token.is_empty() && !token_id.is_empty());

    // The pairing shows up in lan.status.phones (id + label, never the token).
    let status_env = request(&mut root, &Envelope::message("s", "daemon.lan.status", serde_json::json!({})), &mut sink).await;
    let phones = status_env.payload["phones"].as_array().unwrap();
    assert!(phones.iter().any(|p| p["id"] == token_id.as_str()), "pairing missing from status: {phones:?}");
    assert!(
        !status_env.payload.to_string().contains(&phone_token),
        "the token must never appear in lan.status"
    );

    // A phone handshake over the LAN listener with the phone token -> "phone" scope.
    let (mut phone, welcome) = lan_hello(lan_port, &phone_token, ClientKind::Mobile).await;
    assert_eq!(welcome.msg_type, "auth.welcome", "phone auth failed: {welcome:?}");
    assert_eq!(welcome.payload["scope"], "phone");

    // Allowed: read-only fleet status.
    let fleet = request(&mut phone, &Envelope::message("f", "fleet.status", serde_json::json!({})), &mut sink).await;
    assert_eq!(fleet.msg_type, "fleet.status", "phone fleet.status should be allowed: {fleet:?}");
    // Allowed: the Needs You queue.
    let ny = request(&mut phone, &Envelope::message("n", "needs_you.list", serde_json::json!({})), &mut sink).await;
    assert_eq!(ny.msg_type, "needs_you.list");

    // Forbidden: vault, kill, dispatch, agents, recipes, and daemon verbs.
    for (id, verb, payload) in [
        ("x1", "env.set", serde_json::json!({ "project_id": "p", "key": "K", "value": "V", "kind": "secret" })),
        ("x2", "session.kill", serde_json::json!({ "session_id": "01ABC" })),
        ("x3", "dispatch.start", serde_json::json!({ "card_id": "01ABC" })),
        ("x4", "agents.list", serde_json::json!({})),
        ("x5", "recipe.list", serde_json::json!({})),
        ("x6", "daemon.lan.disable", serde_json::json!({})),
        ("x7", "daemon.lan.pair", serde_json::json!({})),
    ] {
        let resp = request(&mut phone, &Envelope::message(id, verb, payload), &mut sink).await;
        assert_eq!(resp.msg_type, "error", "{verb} must be rejected for a phone: {resp:?}");
        assert_eq!(resp.payload["code"], "forbidden", "{verb} must be forbidden: {resp:?}");
    }

    // The LAN listener refuses the ROOT token entirely (loopback-only tokens never work
    // over LAN, a hard boundary beyond the capability gate).
    let (_root_ws, rejected) = lan_hello(lan_port, &token, ClientKind::Desktop).await;
    assert_eq!(rejected.msg_type, "__closed__", "root token must be refused over the LAN: {rejected:?}");

    // Revoke the pairing; the phone token can no longer authenticate.
    let revoked = request(
        &mut root,
        &Envelope::message("rv", "daemon.lan.revoke", serde_json::json!({ "token_id": token_id })),
        &mut sink,
    )
    .await;
    assert_eq!(revoked.payload["revoked"], true, "revoke should report success: {revoked:?}");
    let (_dead_ws, dead) = lan_hello(lan_port, &phone_token, ClientKind::Mobile).await;
    assert_eq!(dead.msg_type, "__closed__", "a revoked phone token must be refused: {dead:?}");

    // And it is gone from the live pairings.
    let after = request(&mut root, &Envelope::message("s2", "daemon.lan.status", serde_json::json!({})), &mut sink).await;
    assert!(
        !after.payload["phones"].as_array().unwrap().iter().any(|p| p["id"] == token_id.as_str()),
        "revoked pairing must drop from the live list"
    );
}
