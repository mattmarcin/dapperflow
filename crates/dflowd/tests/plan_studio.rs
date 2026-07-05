//! Plan Studio artifact service, end to end against a real daemon (`plan-studio.md`,
//! `security.md` / Artifact sandbox, `protocol.md` / artifact.*).
//!
//! Drives the whole review loop over the desktop (root) WS plus raw loopback HTTP:
//! register a plan artifact, serve it over a short-lived signed URL with the exact strict
//! CSP and the injected SDK, reject a tampered signature (403), submit a feedback batch,
//! poll it back (never lost), submit an approve action, and confirm the poll reports the
//! review ended/approved. Deterministic and offline.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use common::*;
use dflow_proto::Envelope;

/// A minimal self-contained plan artifact with a stable anchor id.
const PLAN_HTML: &str = "<!doctype html><html><head><meta charset=utf-8><title>Retry plan</title>\
<style>body{font-family:sans-serif}</style></head><body>\
<h1>Retry plan</h1><p id=\"retry\">retry with exponential backoff</p>\
<form><label>Storage <select name=\"storage\"><option>sqlite</option><option>postgres</option></select></label></form>\
</body></html>";

/// A blocking raw HTTP GET over loopback: returns `(status, headers_lowercased, body)`.
fn http_get(port: u16, path: &str) -> (u16, String, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect http");
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).expect("write http request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read http response");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, head.to_lowercase(), body.to_string())
}

#[tokio::test]
async fn artifact_round_trip_signed_csp_sdk_feedback_and_approve() {
    let data_dir = unique_data_dir("planstudio");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Project + card.
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
            serde_json::json!({ "title": "retry policy", "type": "feature", "project_id": project_id }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Write the plan HTML somewhere the daemon can read.
    let plan_path = data_dir.join("plan.html");
    std::fs::write(&plan_path, PLAN_HTML).unwrap();

    // ---- Register the artifact (round 1). ----
    let reg = request(
        &mut root,
        &Envelope::message(
            "reg",
            "artifact.register",
            serde_json::json!({ "card_id": card_id, "path": plan_path.to_string_lossy(), "kind": "plan", "title": "Retry plan" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(reg.msg_type, "artifact.register", "register failed: {reg:?}");
    assert_eq!(reg.payload["revised"], false);
    let artifact_id = reg.payload["artifact"]["id"].as_str().unwrap().to_string();
    assert_eq!(reg.payload["artifact"]["round"], 1);
    assert_eq!(reg.payload["artifact"]["status"], "open");

    // ---- Get a signed URL and serve the document over HTTP. ----
    let got = request(
        &mut root,
        &Envelope::message("get", "artifact.get", serde_json::json!({ "artifact_id": artifact_id })),
        &mut sink,
    )
    .await;
    let signed_url = got.payload["signed_url"].as_str().unwrap().to_string();
    let path = signed_url.splitn(4, '/').nth(3).map(|p| format!("/{p}")).unwrap();

    let (status, headers, body) = http_get(port, &path);
    assert_eq!(status, 200, "signed URL must serve the document");
    // The exact strict CSP is a real response header (spike 5: honored, not a <meta>).
    assert!(headers.contains("content-security-policy:"), "CSP header present: {headers}");
    assert!(headers.contains("frame-ancestors 'self'"), "frame-ancestors directive present");
    assert!(headers.contains("script-src 'self'"), "script-src directive present");
    assert!(headers.contains("connect-src 'none'"), "connect-src none present");
    // The review SDK is injected server-side as a same-origin script (stamped with the id).
    assert!(body.contains("/artifact/asset/sdk.js"), "SDK script injected: {body}");
    assert!(body.contains(&format!("data-artifact-id=\"{artifact_id}\"")), "artifact id stamped");
    // The original agent HTML is preserved.
    assert!(body.contains("retry with exponential backoff"), "agent content preserved");

    // The injected SDK asset serves and is the self-contained versioned IIFE.
    let (asset_status, _h, asset_body) = http_get(port, "/artifact/asset/sdk.js");
    assert_eq!(asset_status, 200, "the SDK asset serves same-origin");
    assert!(asset_body.contains("dflow.plan.v1"), "the SDK carries the versioned schema");

    // ---- A tampered signature is rejected with 403. ----
    let tampered = if path.ends_with('0') {
        format!("{}1", &path[..path.len() - 1])
    } else {
        format!("{}0", &path[..path.len() - 1])
    };
    let (bad_status, _h, _b) = http_get(port, &tampered);
    assert_eq!(bad_status, 403, "a tampered signature must 403");
    // A garbage signature also 403s.
    let doc_id = path.split('/').nth(3).and_then(|p| p.split('?').next()).unwrap();
    let (garbage_status, _h, _b) = http_get(port, &format!("/artifact/doc/{doc_id}?exp=9999999999999&sig=deadbeef"));
    assert_eq!(garbage_status, 403, "a garbage signature must 403");

    // ---- Submit a feedback batch (round 1) and poll it back. ----
    let submit = request(
        &mut root,
        &Envelope::message(
            "sub1",
            "artifact.feedback.submit",
            serde_json::json!({
                "artifact_id": artifact_id,
                "round": 1,
                "items": [
                    { "kind": "text_range", "anchor": { "selector": "#retry", "start": 0, "end": 30, "quote": "retry with exponential backoff" }, "body": "cap at 3 attempts, then dead-letter", "status": "anchored" },
                    { "kind": "control", "question_key": "storage", "value": "sqlite" }
                ],
                // The layout audit the chrome computed rides along and lands on the record.
                "layout_warnings": [
                    { "selector": "html", "kind": "horizontal_overflow", "overflow_px": 674, "viewport_width": 1200, "severity": "error" }
                ]
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(submit.payload["ok"], true, "submit failed: {submit:?}");

    // Layout audit round trip: the posted findings land on the artifact record and come
    // back on artifact.get.
    let got2 = request(
        &mut root,
        &Envelope::message("get2", "artifact.get", serde_json::json!({ "artifact_id": artifact_id })),
        &mut sink,
    )
    .await;
    let lw = got2.payload["layout_warnings"].as_array().unwrap();
    assert_eq!(lw.len(), 1, "the layout audit landed on the artifact record: {got2:?}");
    assert_eq!(lw[0]["kind"], "horizontal_overflow");
    assert_eq!(lw[0]["severity"], "error");

    // Poll (root passes the explicit artifact id) returns the queued batch.
    let poll = request(
        &mut root,
        &Envelope::message(
            "poll1",
            "artifact.feedback.poll",
            serde_json::json!({ "artifact_id": artifact_id, "wait": true }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(poll.msg_type, "artifact.feedback.poll", "poll failed: {poll:?}");
    assert_eq!(poll.payload["round"], 1);
    assert_eq!(poll.payload["ended"], false);
    let items = poll.payload["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "the batch carries both items: {items:?}");
    assert_eq!(items[0]["kind"], "text_range");
    assert_eq!(items[0]["status"], "anchored", "the anchor status rides the wire");
    assert_eq!(items[1]["kind"], "control");
    assert_eq!(items[1]["value"], "sqlite");
    // The layout audit flows back to the agent through the poll (`plan-studio.md`).
    let poll_lw = poll.payload["layout_warnings"].as_array().unwrap();
    assert_eq!(poll_lw.len(), 1, "the poll carries the layout audit back to the agent");
    assert_eq!(poll_lw[0]["kind"], "horizontal_overflow");

    // A second poll finds nothing queued (the batch was delivered exactly once) -> pending.
    let poll2 = request(
        &mut root,
        &Envelope::message(
            "poll2",
            "artifact.feedback.poll",
            serde_json::json!({ "artifact_id": artifact_id, "wait": false }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(poll2.payload["pending"], true, "no new batch: pending, feedback not double-delivered");

    // ---- Approve: the first-class Approve action ends the review. ----
    let approve = request(
        &mut root,
        &Envelope::message(
            "sub2",
            "artifact.feedback.submit",
            serde_json::json!({
                "artifact_id": artifact_id,
                "round": 1,
                "items": [ { "kind": "action", "action": "approve_plan" } ],
                "layout_warnings": []
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(approve.payload["ok"], true);

    let poll3 = request(
        &mut root,
        &Envelope::message(
            "poll3",
            "artifact.feedback.poll",
            serde_json::json!({ "artifact_id": artifact_id, "wait": true }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(poll3.payload["ended"], true, "approve ends the review: {poll3:?}");
    assert_eq!(poll3.payload["approved"], true);
    assert_eq!(poll3.payload["status"], "approved");

    // ---- card.get surfaces the artifact as ArtifactMeta and the plan_approved event. ----
    let cget = request(
        &mut root,
        &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 200 })),
        &mut sink,
    )
    .await;
    let artifacts = cget.payload["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1, "card.get carries the artifact");
    assert_eq!(artifacts[0]["id"], artifact_id);
    assert_eq!(artifacts[0]["status"], "approved");
    assert!(!artifacts[0]["doc_id"].as_str().unwrap_or("").is_empty(), "the artifact carries a doc_id");
    let events = cget.payload["events"].as_array().unwrap();
    assert!(
        events.iter().any(|e| e["kind"] == "plan_approved"),
        "a plan_approved event is recorded"
    );
    assert!(
        events.iter().any(|e| e["kind"] == "artifact_opened"),
        "an artifact_opened event is recorded"
    );
}
