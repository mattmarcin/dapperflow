//! Evidence capture (opt-in, `#[ignore]`): drive the recipe surfaces against a live
//! daemon and print the exact wire payloads, for the phase4 spike doc.
//!
//! Run: `cargo test -p dflowd --test recipe_evidence -- --ignored --nocapture`

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::{AuthHello, ClientKind, Envelope, PROTOCOL_VERSION};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const STUB: &str = r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%"]"#;

fn pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// Create a card, dispatch it, and print the dispatched + brief_composed payloads.
async fn dispatch_and_show(
    ws: &mut Ws,
    label: &str,
    card_body: serde_json::Value,
    dispatch_extra: serde_json::Value,
) -> String {
    let mut sink = Vec::new();
    let c = request(ws, &Envelope::message(format!("c-{label}"), "card.create", card_body), &mut sink).await;
    let card_id = c.payload["card_id"].as_str().unwrap().to_string();
    let mut body = serde_json::json!({ "card_id": card_id, "harness": "stub" });
    for (k, v) in dispatch_extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    let d = request(ws, &Envelope::message(format!("d-{label}"), "dispatch.start", body), &mut sink).await;
    let g = request(ws, &Envelope::message(format!("g-{label}"), "card.get", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    let payload = g.payload["events"].as_array().unwrap().iter().find(|e| e["kind"] == "dispatched").unwrap()["payload"].clone();
    println!("\n[{label}] dispatch.start -> recipe={} v{}", d.payload["recipe"], d.payload["recipe_version"]);
    println!("dispatched event payload: {}", pretty(&payload));
    let brief = g.payload["events"].as_array().unwrap().iter().find(|e| e["kind"] == "brief_composed").map(|e| e["payload"].clone());
    if let Some(b) = brief {
        println!("brief_composed payload:   {}", pretty(&b));
    }
    card_id
}

#[tokio::test]
#[ignore = "evidence capture; run explicitly with --ignored"]
async fn capture_recipe_evidence() {
    let data_dir = unique_data_dir("recipe-evidence");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) =
        start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB), ("DFLOW_LAUNCH_PI", STUB)]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    println!("======== flow recipes evidence (M2, phase 4) ========");

    // ---- recipe.list ----
    let listed = request(&mut ws, &Envelope::message("rl", "recipe.list", serde_json::json!({})), &mut sink).await;
    println!("\n--- recipe.list ---");
    for r in listed.payload["recipes"].as_array().unwrap() {
        println!(
            "{:12} v{} scope={:8} tier={:10} {}",
            r["name"].as_str().unwrap_or(""),
            r["version"],
            r["scope"].as_str().unwrap_or(""),
            r["trust_tier"].as_str().unwrap_or(""),
            r["description"].as_str().unwrap_or("")
        );
    }

    // ---- recipe.get audit-deep: extends resolved ----
    let got = request(&mut ws, &Envelope::message("rg", "recipe.get", serde_json::json!({ "name": "audit-deep" })), &mut sink).await;
    println!("\n--- recipe.get audit-deep (extends: audit resolved) ---");
    println!("stages   = {}", got.payload["parsed"]["stages"]);
    println!("budgets  = {}", got.payload["parsed"]["budgets"]);
    println!("extends  = {} (cleared after resolution)", got.payload["parsed"]["extends"]);

    // ---- recipe.validate: precise errors ----
    println!("\n--- recipe.validate: precise errors ---");
    for (label, content) in [
        ("unknown stage", "---\nname: broken\nversion: 1\nstages: [design]\n---\n"),
        ("bad worktree value", "---\nname: b\nversion: 1\nstages: [implement]\nimplement:\n  worktree: yolo\n---\n"),
        ("missing version", "---\nname: b\nstages: [implement]\n---\n"),
        ("tab indentation", "---\nname: b\nversion: 1\nstages: [implement]\nbudgets:\n\tcards: 5\n---\n"),
    ] {
        let v = request(&mut ws, &Envelope::message(format!("rv-{label}"), "recipe.validate", serde_json::json!({ "content": content })), &mut sink).await;
        let err = &v.payload["errors"][0];
        println!("{label:20} -> line={} message={}", err["line"], err["message"]);
    }
    let privileged = request(
        &mut ws,
        &Envelope::message("rvp", "recipe.validate", serde_json::json!({ "content": "---\nname: p\nversion: 1\nstages: [implement, ship]\nimplement:\n  worktree: in_place\nship:\n  target: local_merge\n---\n" })),
        &mut sink,
    )
    .await;
    println!("privileged classify  -> tier={} elevations={}", privileged.payload["trust_tier"], privileged.payload["elevations"]);

    // ---- resolution precedence through real dispatches ----
    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    println!("\n--- dispatch resolution precedence (dispatched event payloads) ---");
    let _ = dispatch_and_show(
        &mut ws,
        "1 global default",
        serde_json::json!({ "title": "one", "type": "chore", "project_id": project_id }),
        serde_json::json!({}),
    )
    .await;

    let _ = request(&mut ws, &Envelope::message("pu", "project.update", serde_json::json!({ "project_id": project_id, "default_recipe": "presto" })), &mut sink).await;
    let _ = dispatch_and_show(
        &mut ws,
        "2 project default_recipe=presto",
        serde_json::json!({ "title": "two", "type": "chore", "project_id": project_id }),
        serde_json::json!({}),
    )
    .await;

    let audit_card = dispatch_and_show(
        &mut ws,
        "3 card dial_recipe=audit",
        serde_json::json!({ "title": "Onboard repo", "type": "investigation", "project_id": project_id, "dial_recipe": "audit" }),
        serde_json::json!({ "audit": true }),
    )
    .await;

    let _ = dispatch_and_show(
        &mut ws,
        "4 explicit param recipe=deep over dial",
        serde_json::json!({ "title": "four", "type": "feature", "project_id": project_id, "dial_recipe": "audit-deep" }),
        serde_json::json!({ "recipe": "deep" }),
    )
    .await;

    // ---- audit budgets on the agent surface ----
    println!("\n--- audit recipe budgets (10 cards / 6 notes from the recipe file) ---");
    let cget = request(&mut ws, &Envelope::message("ag", "card.get", serde_json::json!({ "card_id": audit_card })), &mut sink).await;
    let session_id = cget.payload["sessions"][0]["session_id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(&mut ws, &Envelope::message("at", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })), &mut sink).await;
    let replay = base64::engine::general_purpose::STANDARD.decode(attached.payload["replay_base64"].as_str().unwrap()).unwrap();
    let screen = String::from_utf8_lossy(&replay);
    let start = screen.find("DTOKEN=").unwrap() + 7;
    let task_token: String = screen[start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();

    let url = format!("ws://127.0.0.1:{port}/ws");
    let (mut agent, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let hello = Envelope::message("auth", "auth.hello", AuthHello { token: task_token, client: ClientKind::Agent, proto_versions: vec![PROTOCOL_VERSION] });
    agent.send(WsMessage::Text(serde_json::to_string(&hello).unwrap().into())).await.unwrap();
    let _ = next_envelope(&mut agent).await;

    for i in 0..10 {
        let cc = request(&mut agent, &Envelope::message(format!("cc{i}"), "card.create", serde_json::json!({ "title": format!("finding {i}"), "type": "bug", "fingerprint": format!("src/mod{i}.rs:finding") })), &mut sink).await;
        if i == 0 || i == 9 {
            println!("create {} -> {} lane={} origin={}", i + 1, cc.msg_type, cc.payload["card"]["lane"], cc.payload["card"]["origin_kind"]);
        }
    }
    let over = request(&mut agent, &Envelope::message("over", "card.create", serde_json::json!({ "title": "one too many", "type": "bug" })), &mut sink).await;
    println!("create 11 -> error: {}", pretty(&over.payload));
    let done = request(&mut agent, &Envelope::message("done", "session.self_report", serde_json::json!({ "state": "done", "note": "report written" })), &mut sink).await;
    println!("dflow status done -> {}", pretty(&done.payload));

    // ---- privileged grant flow ----
    println!("\n--- privileged recipe: consent, grant, hash invalidation ---");
    let shipfast = "---\nname: shipfast\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: checks_only\nship:\n  target: local_merge\n---\n\n## implement\nGo fast.\n";
    let inst = request(&mut ws, &Envelope::message("ri", "recipe.install", serde_json::json!({ "source": "shipfast.md", "scope": "project", "project_id": project_id, "content": shipfast })), &mut sink).await;
    println!("recipe.install -> {}", pretty(&inst.payload));
    let card = request(&mut ws, &Envelope::message("pc", "card.create", serde_json::json!({ "title": "risky", "type": "chore", "project_id": project_id, "dial_recipe": "shipfast" })), &mut sink).await;
    let card_id = card.payload["card_id"].as_str().unwrap().to_string();
    let refused = request(&mut ws, &Envelope::message("pd1", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    println!("dispatch without grant -> {}", pretty(&refused.payload));
    let granted = request(&mut ws, &Envelope::message("pg", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "shipfast" })), &mut sink).await;
    println!("recipe.grant -> {}", pretty(&granted.payload));
    let ok = request(&mut ws, &Envelope::message("pd2", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    println!("dispatch with grant -> {} recipe={}", ok.msg_type, ok.payload["recipe"]);
    let path = inst.payload["path"].as_str().unwrap();
    std::fs::write(path, shipfast.replace("Go fast.", "Go faster.")).unwrap();
    let stale = request(&mut ws, &Envelope::message("pd3", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    println!("dispatch after file edit -> {}", pretty(&stale.payload));

    // ---- MCP x harness validation ----
    println!("\n--- MCP x harness validation (pi rejects at dispatch) ---");
    let mcp = "---\nname: withmcp\nversion: 1\nstages: [implement]\nmcp:\n  - name: context7\n    command: \"npx -y context7\"\n---\n";
    let _ = request(&mut ws, &Envelope::message("ri2", "recipe.install", serde_json::json!({ "source": "withmcp.md", "scope": "project", "project_id": project_id, "content": mcp })), &mut sink).await;
    let _ = request(&mut ws, &Envelope::message("pg2", "recipe.grant", serde_json::json!({ "project_id": project_id, "recipe_name": "withmcp" })), &mut sink).await;
    let mc = request(&mut ws, &Envelope::message("mc", "card.create", serde_json::json!({ "title": "docs", "type": "chore", "project_id": project_id, "dial_recipe": "withmcp" })), &mut sink).await;
    let mcard = mc.payload["card_id"].as_str().unwrap().to_string();
    let on_pi = request(&mut ws, &Envelope::message("md", "dispatch.start", serde_json::json!({ "card_id": mcard, "harness": "pi" })), &mut sink).await;
    println!("dispatch withmcp on pi -> {}", pretty(&on_pi.payload));

    let _ = request(&mut ws, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
    println!("\n======== end evidence ========");
}
