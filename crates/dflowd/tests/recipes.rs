//! Recipe-driven dispatch end to end (`recipes.md`, `security.md`, `protocol.md`):
//! the recipe.* verbs, the card > project > global resolution precedence proven
//! through real dispatches, recipe budgets enforced on the agent surface, the
//! privileged-recipe grant flow with hash invalidation, the in_place ack, and
//! dispatch-time MCP x harness validation. All against a real daemon with a stub
//! harness (`DFLOW_LAUNCH_*` seam), deterministically and offline.

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The stub echoes its injected token env into the PTY, then stays alive.
const STUB_LAUNCH: &str =
    r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%; DEND=%DFLOW_ENDPOINT%; DCARD=%DFLOW_CARD%"]"#;

/// Open an `agent`-client WS connection authenticated with a per-task token.
async fn connect_agent(port: u16, token: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.expect("connect");
    let hello = Envelope::message(
        "auth",
        "auth.hello",
        AuthHello {
            token: token.to_string(),
            client: ClientKind::Agent,
            proto_versions: vec![PROTOCOL_VERSION],
        },
    );
    ws.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let welcome = next_envelope(&mut ws).await;
    assert_eq!(welcome.msg_type, "auth.welcome", "agent auth failed: {welcome:?}");
    ws
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

/// Fetch the newest `dispatched` event payload for a card.
async fn dispatched_payload(ws: &mut Ws, card_id: &str, tag: &str) -> serde_json::Value {
    let mut sink = Vec::new();
    let got = request(
        ws,
        &Envelope::message(tag.to_string(), "card.get", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    got.payload["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "dispatched")
        .unwrap_or_else(|| panic!("no dispatched event for card {card_id}"))["payload"]
        .clone()
}

/// Create a card and return its id.
async fn create_card(ws: &mut Ws, tag: &str, body: serde_json::Value) -> String {
    let mut sink = Vec::new();
    let resp = request(ws, &Envelope::message(tag.to_string(), "card.create", body), &mut sink).await;
    assert_eq!(resp.msg_type, "card.create", "card.create failed: {resp:?}");
    resp.payload["card_id"].as_str().unwrap().to_string()
}

/// Dispatch a card and return the response envelope (success or error).
async fn dispatch(ws: &mut Ws, tag: &str, body: serde_json::Value) -> Envelope {
    let mut sink = Vec::new();
    request(ws, &Envelope::message(tag.to_string(), "dispatch.start", body), &mut sink).await
}

// ---------------------------------------------------------------------------

/// recipe.list / recipe.get / recipe.validate, then the resolution precedence proven
/// through four dispatches: standard (global default) < project default_recipe < card
/// dial < explicit dispatch parameter, each recorded in the dispatched event.
#[tokio::test]
async fn recipe_verbs_and_resolution_precedence() {
    let data_dir = unique_data_dir("recipes");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // ---- recipe.list: the five bundled recipes, all standard tier. ----
    let listed = request(&mut ws, &Envelope::message("rl", "recipe.list", serde_json::json!({})), &mut sink).await;
    assert_eq!(listed.msg_type, "recipe.list", "recipe.list failed: {listed:?}");
    let recipes = listed.payload["recipes"].as_array().unwrap();
    let names: Vec<&str> = recipes.iter().filter_map(|r| r["name"].as_str()).collect();
    for expected in ["presto", "standard", "deep", "audit", "audit-deep"] {
        assert!(names.contains(&expected), "bundled recipe '{expected}' missing: {names:?}");
    }
    for r in recipes {
        assert_eq!(r["scope"], "bundled");
        assert_eq!(r["trust_tier"], "standard", "bundled recipes have no special powers: {r:?}");
    }
    assert!(listed.payload["invalid"].as_array().unwrap().is_empty());

    // ---- recipe.get: audit-deep resolves through extends with raised budgets. ----
    let got = request(&mut ws, &Envelope::message("rg", "recipe.get", serde_json::json!({ "name": "audit-deep" })), &mut sink).await;
    assert_eq!(got.payload["found"], true);
    assert_eq!(got.payload["parsed"]["budgets"]["cards"], 25);
    assert_eq!(got.payload["parsed"]["budgets"]["notes"], 12);
    assert_eq!(got.payload["parsed"]["stages"], serde_json::json!(["implement"]));
    assert!(got.payload["parsed"]["extends"].is_null(), "extends is resolved away");

    // ---- recipe.validate: precise errors, no partial application. ----
    let bad = request(
        &mut ws,
        &Envelope::message(
            "rv1",
            "recipe.validate",
            serde_json::json!({ "content": "---\nname: broken\nversion: 1\nstages: [design]\n---\n" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(bad.payload["valid"], false);
    let msg = bad.payload["errors"][0]["message"].as_str().unwrap();
    assert!(msg.contains("unknown stage 'design'"), "imprecise error: {msg}");
    assert!(msg.contains("shape, plan, implement, verify, ship"), "error should list the vocabulary: {msg}");

    let bad2 = request(
        &mut ws,
        &Envelope::message(
            "rv2",
            "recipe.validate",
            serde_json::json!({ "content": "---\nname: b2\nversion: 1\nstages: [implement]\nimplement:\n  worktree: yolo\n---\n" }),
        ),
        &mut sink,
    )
    .await;
    let msg2 = bad2.payload["errors"][0]["message"].as_str().unwrap();
    assert!(msg2.contains("implement.worktree"), "error must name the field: {msg2}");

    // A privileged recipe classifies with its elevations disclosed.
    let priv_check = request(
        &mut ws,
        &Envelope::message(
            "rv3",
            "recipe.validate",
            serde_json::json!({ "content": "---\nname: p\nversion: 1\nstages: [implement, ship]\nimplement:\n  worktree: in_place\nship:\n  target: local_merge\n---\n" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(priv_check.payload["valid"], true);
    assert_eq!(priv_check.payload["trust_tier"], "privileged");
    let elevations = priv_check.payload["elevations"].as_array().unwrap();
    assert_eq!(elevations.len(), 2, "in_place + local_merge: {elevations:?}");

    // ---- Resolution precedence, proven through dispatched events. ----
    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    // 1. Nothing selects a recipe: the global default `standard` wins.
    let card1 = create_card(&mut ws, "c1", serde_json::json!({ "title": "one", "type": "chore", "project_id": project_id })).await;
    let d1 = dispatch(&mut ws, "d1", serde_json::json!({ "card_id": card1, "harness": "stub" })).await;
    assert_eq!(d1.msg_type, "dispatch.start", "dispatch 1 failed: {d1:?}");
    assert_eq!(d1.payload["recipe"], "standard");
    let p1 = dispatched_payload(&mut ws, &card1, "g1").await;
    assert_eq!(p1["recipe"], "standard");
    assert_eq!(p1["recipe_version"], 1);
    assert_eq!(p1["recipe_scope"], "bundled");
    assert_eq!(p1["worktree_strategy"], "pooled");

    // 2. The project default_recipe wins over the global default.
    let upd = request(&mut ws, &Envelope::message("pu", "project.update", serde_json::json!({ "project_id": project_id, "default_recipe": "presto" })), &mut sink).await;
    assert_eq!(upd.msg_type, "project.update");
    let card2 = create_card(&mut ws, "c2", serde_json::json!({ "title": "two", "type": "chore", "project_id": project_id })).await;
    let d2 = dispatch(&mut ws, "d2", serde_json::json!({ "card_id": card2, "harness": "stub" })).await;
    assert_eq!(d2.payload["recipe"], "presto", "project default must win: {d2:?}");

    // 3. The card's dial wins over the project default; audit-deep's inherited budgets
    //    flow into the dispatch.
    let card3 = create_card(&mut ws, "c3", serde_json::json!({ "title": "three", "type": "investigation", "project_id": project_id, "dial_recipe": "audit-deep" })).await;
    let d3 = dispatch(&mut ws, "d3", serde_json::json!({ "card_id": card3, "harness": "stub" })).await;
    assert_eq!(d3.payload["recipe"], "audit-deep", "card dial must win: {d3:?}");
    let p3 = dispatched_payload(&mut ws, &card3, "g3").await;
    assert_eq!(p3["budgets"]["cards"], 25, "audit-deep budgets flow through dispatch: {p3}");
    assert_eq!(p3["budgets"]["notes"], 12);

    // 4. An explicit dispatch parameter wins over the card dial.
    let card4 = create_card(&mut ws, "c4", serde_json::json!({ "title": "four", "type": "feature", "project_id": project_id, "dial_recipe": "audit-deep" })).await;
    let d4 = dispatch(&mut ws, "d4", serde_json::json!({ "card_id": card4, "harness": "stub", "recipe": "deep" })).await;
    assert_eq!(d4.payload["recipe"], "deep", "explicit parameter must win: {d4:?}");

    // The brief_composed event proves the recipe guidance reached the brief.
    let mut sink2 = Vec::new();
    let got4 = request(&mut ws, &Envelope::message("g4", "card.get", serde_json::json!({ "card_id": card4 })), &mut sink2).await;
    let brief_ev = got4.payload["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "brief_composed")
        .expect("brief_composed event missing");
    assert_eq!(brief_ev["payload"]["recipe"], "deep");
    let stages = brief_ev["payload"]["guidance_stages"].as_array().unwrap();
    let stage_names: Vec<&str> = stages.iter().filter_map(|s| s.as_str()).collect();
    assert!(stage_names.contains(&"plan") && stage_names.contains(&"implement"),
        "deep guidance stages injected into the brief: {stage_names:?}");

    // An unknown recipe name refuses cleanly.
    let card5 = create_card(&mut ws, "c5", serde_json::json!({ "title": "five", "type": "chore", "project_id": project_id })).await;
    let d5 = dispatch(&mut ws, "d5", serde_json::json!({ "card_id": card5, "harness": "stub", "recipe": "ghost" })).await;
    assert_eq!(d5.msg_type, "error");
    assert_eq!(d5.payload["code"], "not_found");

    let _ = request(&mut ws, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}

/// The audit recipe's budgets (10 cards / 6 notes) flow from the recipe file into the
/// per-task token and are engine-enforced on the agent surface; audit cards land in
/// Inbox and the audit token can never move their lanes (`security.md`).
#[tokio::test]
async fn audit_recipe_budgets_enforced() {
    let data_dir = unique_data_dir("auditbudget");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut root, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    // The audit card: investigation-shaped, audit dial, audit-scoped token.
    let card_id = create_card(&mut root, "c", serde_json::json!({ "title": "Onboard repo", "type": "investigation", "project_id": project_id, "dial_recipe": "audit" })).await;
    let disp = dispatch(&mut root, "d", serde_json::json!({ "card_id": card_id, "harness": "stub", "audit": true })).await;
    assert_eq!(disp.msg_type, "dispatch.start", "audit dispatch failed: {disp:?}");
    assert_eq!(disp.payload["recipe"], "audit");
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    // The dispatched event records the recipe's budgets (no explicit params given).
    let payload = dispatched_payload(&mut root, &card_id, "g").await;
    assert_eq!(payload["budgets"]["cards"], 10, "audit budget from the recipe file: {payload}");
    assert_eq!(payload["budgets"]["notes"], 6);

    // Read the injected task token back from the stub session's scrollback.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(&mut root, &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })), &mut sink).await;
    let replay = base64::engine::general_purpose::STANDARD
        .decode(attached.payload["replay_base64"].as_str().unwrap())
        .unwrap();
    let task_token = extract_token(&String::from_utf8_lossy(&replay)).expect("token in scrollback");
    let mut agent = connect_agent(port, &task_token).await;

    // Ten creates succeed (the recipe budget); every one lands in Inbox.
    let mut last_id = String::new();
    for i in 0..10 {
        let cc = request(
            &mut agent,
            &Envelope::message(
                format!("cc{i}"),
                "card.create",
                serde_json::json!({ "title": format!("finding {i}"), "type": "bug", "fingerprint": format!("src/f{i}.rs:finding-{i}") }),
            ),
            &mut sink,
        )
        .await;
        assert_eq!(cc.msg_type, "card.create", "audit create {i} refused early: {cc:?}");
        assert_eq!(cc.payload["card"]["lane"], "inbox", "audit cards land in Inbox only");
        assert_eq!(cc.payload["card"]["origin_kind"], "audit");
        last_id = cc.payload["card_id"].as_str().unwrap().to_string();
    }

    // The eleventh returns the structured budget error pointing at the report.
    let over = request(&mut agent, &Envelope::message("over", "card.create", serde_json::json!({ "title": "one too many", "type": "bug" })), &mut sink).await;
    assert_eq!(over.msg_type, "error");
    assert_eq!(over.payload["code"], "budget_exceeded");
    assert!(over.payload["message"].as_str().unwrap().contains("report"),
        "budget error should send the remainder to the report: {over:?}");

    // The audit token can never advance its own filings (`security.md`).
    let mv = request(&mut agent, &Envelope::message("mv", "card.move", serde_json::json!({ "card_id": last_id, "column": "ready" })), &mut sink).await;
    assert_eq!(mv.msg_type, "error");
    assert_eq!(mv.payload["code"], "forbidden");

    // `dflow status done` arbitration answers with the audit recipe's stage list.
    let done = request(&mut agent, &Envelope::message("done", "session.self_report", serde_json::json!({ "state": "done", "note": "report written" })), &mut sink).await;
    assert_eq!(done.payload["recorded"], "done");
    assert_eq!(done.payload["advanced"], true);
    let next = done.payload["next"].as_str().unwrap();
    assert!(next.contains("recipe audit v1") && next.contains("ends at implement"),
        "done arbitration should speak the recipe's stage list: {next}");

    let _ = request(&mut root, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}

/// Privileged recipes: dispatch refuses without a grant (structured consent error),
/// a grant is bound to the file hash and invalidates on edit, in_place additionally
/// demands an explicit ack, and an MCP-mounting recipe fails on pi at dispatch time.
#[tokio::test]
async fn privileged_grants_in_place_and_mcp_validation() {
    let data_dir = unique_data_dir("privileged");
    let repo = scratch_repo(&data_dir);
    // A pi launch override so the harness resolves; MCP validation must reject BEFORE
    // any launch happens, so the argv is never actually spawned.
    let (_daemon, port, token) = start_daemon(
        &data_dir,
        &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH), ("DFLOW_LAUNCH_PI", STUB_LAUNCH)],
    );
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let project_path = padd.payload["project"]["path"].as_str().unwrap().to_string();

    // ---- Install a privileged project recipe (ship: local_merge). ----
    let shipfast = "---\nname: shipfast\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: checks_only\nship:\n  target: local_merge\n---\n\n## implement\nGo fast.\n";
    let installed = request(
        &mut ws,
        &Envelope::message("ri", "recipe.install", serde_json::json!({ "source": "shipfast.md", "scope": "project", "project_id": project_id, "content": shipfast })),
        &mut sink,
    )
    .await;
    assert_eq!(installed.msg_type, "recipe.install", "install failed: {installed:?}");
    assert_eq!(installed.payload["trust_tier"], "privileged");
    let installed_path = installed.payload["path"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&installed_path).exists());

    // An invalid recipe is never installed ("invalid recipes never partially apply").
    let bad_install = request(
        &mut ws,
        &Envelope::message("rib", "recipe.install", serde_json::json!({ "source": "broken.md", "scope": "project", "project_id": project_id, "content": "---\nname: broken\nversion: 1\nstages: [nope]\n---\n" })),
        &mut sink,
    )
    .await;
    assert_eq!(bad_install.msg_type, "error");
    assert!(!repo.join(".dapperflow/recipes/broken.md").exists(), "invalid recipe must not land on disk");

    // ---- Dispatch without a grant: structured consent error, no session. ----
    let card1 = create_card(&mut ws, "c1", serde_json::json!({ "title": "risky", "type": "chore", "project_id": project_id, "dial_recipe": "shipfast" })).await;
    let refused = dispatch(&mut ws, "d1", serde_json::json!({ "card_id": card1, "harness": "stub" })).await;
    assert_eq!(refused.msg_type, "error");
    assert_eq!(refused.payload["code"], "consent_required", "got: {refused:?}");
    let detail: serde_json::Value =
        serde_json::from_str(refused.payload["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detail["recipe_name"], "shipfast");
    assert_eq!(detail["reason"], "no_grant");
    let elevations = detail["elevations"].as_array().unwrap();
    assert!(elevations.iter().any(|e| e.as_str().unwrap().contains("local merge")),
        "the consent payload lists exactly what is elevated: {elevations:?}");

    // ---- Grant, then dispatch succeeds. ----
    let granted = request(&mut ws, &Envelope::message("gr", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "shipfast" })), &mut sink).await;
    assert_eq!(granted.msg_type, "recipe.grant", "grant failed: {granted:?}");
    let granted_hash = granted.payload["recipe_hash"].as_str().unwrap().to_string();
    assert!(!granted_hash.is_empty());

    let ok = dispatch(&mut ws, "d2", serde_json::json!({ "card_id": card1, "harness": "stub" })).await;
    assert_eq!(ok.msg_type, "dispatch.start", "granted dispatch failed: {ok:?}");
    assert_eq!(ok.payload["recipe"], "shipfast");

    // ---- Edit the recipe file: the hash changes, the grant invalidates. ----
    std::fs::write(&installed_path, shipfast.replace("Go fast.", "Go faster.")).unwrap();
    let card2 = create_card(&mut ws, "c2", serde_json::json!({ "title": "risky 2", "type": "chore", "project_id": project_id, "dial_recipe": "shipfast" })).await;
    let stale = dispatch(&mut ws, "d3", serde_json::json!({ "card_id": card2, "harness": "stub" })).await;
    assert_eq!(stale.msg_type, "error");
    assert_eq!(stale.payload["code"], "consent_required");
    let detail: serde_json::Value = serde_json::from_str(stale.payload["detail"].as_str().unwrap()).unwrap();
    assert_eq!(detail["reason"], "hash_changed", "an edited file must force re-confirmation");

    // Re-granting under the new hash restores dispatchability.
    let regrant = request(&mut ws, &Envelope::message("gr2", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "shipfast" })), &mut sink).await;
    assert_ne!(regrant.payload["recipe_hash"].as_str().unwrap(), granted_hash, "grant re-binds to the new hash");
    let ok2 = dispatch(&mut ws, "d4", serde_json::json!({ "card_id": card2, "harness": "stub" })).await;
    assert_eq!(ok2.msg_type, "dispatch.start", "re-granted dispatch failed: {ok2:?}");

    // Granting a standard recipe is refused (no-op grants train click-through).
    let noop = request(&mut ws, &Envelope::message("gr3", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "standard" })), &mut sink).await;
    assert_eq!(noop.msg_type, "error");
    assert_eq!(noop.payload["code"], "bad_request");

    // ---- in_place: grant alone is not enough; an explicit ack is also required. ----
    let inplace = "---\nname: inplace\nversion: 1\nstages: [implement]\nimplement:\n  worktree: in_place\n---\n\n## implement\nEdit in place, deliberately.\n";
    let inst2 = request(
        &mut ws,
        &Envelope::message("ri2", "recipe.install", serde_json::json!({ "source": "inplace.md", "scope": "project", "project_id": project_id, "content": inplace })),
        &mut sink,
    )
    .await;
    assert_eq!(inst2.payload["trust_tier"], "privileged");
    let g2 = request(&mut ws, &Envelope::message("gr4", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "inplace" })), &mut sink).await;
    assert_eq!(g2.msg_type, "recipe.grant");

    let card3 = create_card(&mut ws, "c3", serde_json::json!({ "title": "in place", "type": "chore", "project_id": project_id, "dial_recipe": "inplace" })).await;
    let no_ack = dispatch(&mut ws, "d5", serde_json::json!({ "card_id": card3, "harness": "stub" })).await;
    assert_eq!(no_ack.msg_type, "error");
    assert!(no_ack.payload["message"].as_str().unwrap().contains("ack_in_place"),
        "in_place without the ack must name the missing flag: {no_ack:?}");

    let acked = dispatch(&mut ws, "d6", serde_json::json!({ "card_id": card3, "harness": "stub", "ack_in_place": true })).await;
    assert_eq!(acked.msg_type, "dispatch.start", "acked in_place dispatch failed: {acked:?}");
    assert_eq!(acked.payload["worktree_id"], "", "in_place takes no lease");
    assert_eq!(acked.payload["worktree_path"].as_str().unwrap(), project_path,
        "in_place runs in the project checkout itself");

    // ---- MCP x harness: an MCP-mounting recipe fails on pi AT DISPATCH. ----
    let mcp = "---\nname: withmcp\nversion: 1\nstages: [implement]\nmcp:\n  - name: context7\n    command: \"npx -y context7\"\n---\n\n## implement\nUse the docs server.\n";
    let inst3 = request(
        &mut ws,
        &Envelope::message("ri3", "recipe.install", serde_json::json!({ "source": "withmcp.md", "scope": "project", "project_id": project_id, "content": mcp })),
        &mut sink,
    )
    .await;
    assert_eq!(inst3.payload["trust_tier"], "privileged", "mcp mounts are privileged");
    let g3 = request(&mut ws, &Envelope::message("gr5", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "withmcp" })), &mut sink).await;
    assert_eq!(g3.msg_type, "recipe.grant");

    let card4 = create_card(&mut ws, "c4", serde_json::json!({ "title": "docs", "type": "chore", "project_id": project_id, "dial_recipe": "withmcp" })).await;
    let on_pi = dispatch(&mut ws, "d7", serde_json::json!({ "card_id": card4, "harness": "pi" })).await;
    assert_eq!(on_pi.msg_type, "error");
    assert!(on_pi.payload["message"].as_str().unwrap().contains("no verified MCP support"),
        "pi must reject an MCP-mounting recipe: {on_pi:?}");

    // The declared mounts are recorded as meta on a successful dispatch (mounting is
    // deferred to the Concertmaster phase): dispatch the same recipe on the stub? The
    // stub family has no manifest, so it is also rejected; record-keeping is proven at
    // the unit level (api::tests) and by the dispatched-event schema on granted flows.

    let _ = request(&mut ws, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}
