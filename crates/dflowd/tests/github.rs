//! Integration tests for the GitHub transport + issue import (`roadmap.md` M5.1-2,
//! `product.md` / Card sources: GitHub issue import).
//!
//! These drive the REAL gh transport subprocess path: the daemon points `DFLOW_GH` at
//! the compiled `stub-gh` binary, so arg construction, spawn, stdout capture, `--json`
//! parse, and exit-code handling all run against a real `gh`-shaped program. The stub is
//! env-driven and stateful; no live GitHub is touched.

mod common;

use std::path::PathBuf;

use common::*;
use dflow_proto::Envelope;

fn stub_gh() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stub-gh"))
}

/// Two issues: a bug (labelled) and a feature (labelled).
fn issues_fixture() -> serde_json::Value {
    serde_json::json!([
        {
            "number": 12, "title": "Login returns 500", "body": "Steps to repro: click login.",
            "state": "OPEN", "url": "https://github.com/acme/web/issues/12",
            "labels": [{"name": "bug"}, {"name": "p1"}], "assignees": [{"login": "alice"}],
            "milestone": {"title": "v1"}
        },
        {
            "number": 15, "title": "Add CSV export", "body": "Users want CSV.",
            "state": "OPEN", "url": "https://github.com/acme/web/issues/15",
            "labels": [{"name": "enhancement"}], "assignees": [], "milestone": null
        }
    ])
}

/// The env that points the daemon's gh transport at the stub with a canned repo + issues.
fn stub_env<'a>(gh: &'a str, issues_path: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("DFLOW_GH", gh),
        ("DFLOW_STUB_GH_AUTHED", "1"),
        ("DFLOW_STUB_GH_ACCOUNT", "tester"),
        ("DFLOW_STUB_GH_REPO", "acme/web/main"),
        ("DFLOW_STUB_GH_ISSUES", issues_path),
    ]
}

#[tokio::test]
async fn issue_import_dedupes_and_respects_local_moves() {
    let gh = stub_gh();
    if !gh.exists() {
        eprintln!("SKIP: stub-gh not built ({}); run under `cargo test --workspace`", gh.display());
        return;
    }
    let data_dir = unique_data_dir("ghimport");
    let repo = scratch_repo(&data_dir);
    let issues_path = data_dir.join("issues.json");
    std::fs::write(&issues_path, issues_fixture().to_string()).unwrap();

    let gh_str = gh.to_string_lossy().into_owned();
    let issues_str = issues_path.to_string_lossy().into_owned();
    let env = stub_env(&gh_str, &issues_str);
    let (_daemon, port, token) = start_daemon(&data_dir, &env);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // gh presence/auth is reported, not an OAuth flow.
    let auth = request(&mut ws, &Envelope::message("auth", "github.auth.status", serde_json::json!({})), &mut sink).await;
    assert_eq!(auth.msg_type, "github.auth.status", "auth status failed: {auth:?}");
    assert_eq!(auth.payload["present"], true);
    assert_eq!(auth.payload["authenticated"], true);
    assert_eq!(auth.payload["account"], "tester");

    // Register the project.
    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    // Preview: both issues are new (no cards yet), read-only.
    let preview = request(&mut ws, &Envelope::message("pv", "github.issues.preview", serde_json::json!({ "project_id": project_id })), &mut sink).await;
    assert_eq!(preview.msg_type, "github.issues.preview", "preview failed: {preview:?}");
    assert_eq!(preview.payload["repo"], "acme/web");
    let pv_issues = preview.payload["issues"].as_array().unwrap();
    assert_eq!(pv_issues.len(), 2);
    assert!(pv_issues.iter().all(|i| i["dedupe"] == "new"));
    // Preview created nothing.
    let cards0 = request(&mut ws, &Envelope::message("q0", "card.query", serde_json::json!({ "filter": { "project_id": project_id } })), &mut sink).await;
    assert_eq!(cards0.payload["cards"].as_array().unwrap().len(), 0, "preview must not create cards");

    // Import: two origin cards, typed by label heuristics (bug / feature).
    let imp = request(&mut ws, &Envelope::message("im", "github.issues.import", serde_json::json!({ "project_id": project_id })), &mut sink).await;
    assert_eq!(imp.msg_type, "github.issues.import", "import failed: {imp:?}");
    let results = imp.payload["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r["outcome"] == "created"));

    let cards = request(&mut ws, &Envelope::message("q", "card.query", serde_json::json!({ "filter": { "project_id": project_id } })), &mut sink).await;
    let cards = cards.payload["cards"].as_array().unwrap().clone();
    assert_eq!(cards.len(), 2, "one card per issue");
    let bug = cards.iter().find(|c| c["title"] == "Login returns 500").unwrap();
    let feat = cards.iter().find(|c| c["title"] == "Add CSV export").unwrap();
    assert_eq!(bug["type"], "bug", "a `bug`-labelled issue is typed bug");
    assert_eq!(feat["type"], "feature", "an `enhancement`-labelled issue is typed feature");
    assert_eq!(bug["origin_kind"], "github_issue");
    assert_eq!(bug["origin_ref"], "acme/web#12");
    assert_eq!(bug["lane"], "inbox");
    let bug_id = bug["id"].as_str().unwrap().to_string();
    let feat_id = feat["id"].as_str().unwrap().to_string();

    // The Issue tab reads the snapshot: body + labels + assignees + milestone.
    let issue = request(&mut ws, &Envelope::message("ig", "github.issue.get", serde_json::json!({ "card_id": bug_id })), &mut sink).await;
    assert_eq!(issue.msg_type, "github.issue.get", "issue.get failed: {issue:?}");
    assert_eq!(issue.payload["issue"]["number"], 12);
    assert_eq!(issue.payload["issue"]["body"], "Steps to repro: click login.");
    let labels = issue.payload["issue"]["labels"].as_array().unwrap();
    assert!(labels.iter().any(|l| l == "bug") && labels.iter().any(|l| l == "p1"));
    assert_eq!(issue.payload["issue"]["assignees"][0], "alice");
    assert_eq!(issue.payload["issue"]["milestone"], "v1");

    // A LOCAL lane move: the human pulls the bug into Performing.
    let mv = request(&mut ws, &Envelope::message("mv", "card.move", serde_json::json!({ "card_id": bug_id, "column": "performing" })), &mut sink).await;
    assert_eq!(mv.payload["card"]["lane"], "performing");

    // The issue title changes upstream; re-import refreshes fields but respects the move.
    let mut refreshed = issues_fixture();
    refreshed[0]["title"] = serde_json::Value::String("Login returns 500 (updated)".into());
    std::fs::write(&issues_path, refreshed.to_string()).unwrap();

    let imp2 = request(&mut ws, &Envelope::message("im2", "github.issues.import", serde_json::json!({ "project_id": project_id })), &mut sink).await;
    let results2 = imp2.payload["results"].as_array().unwrap();
    assert!(results2.iter().all(|r| r["outcome"] == "refreshed"), "re-import refreshes: {results2:?}");
    // Still exactly two cards (dedupe).
    let cards2 = request(&mut ws, &Envelope::message("q2", "card.query", serde_json::json!({ "filter": { "project_id": project_id } })), &mut sink).await;
    let cards2 = cards2.payload["cards"].as_array().unwrap().clone();
    assert_eq!(cards2.len(), 2, "re-import dedupes, never duplicates");
    let bug2 = cards2.iter().find(|c| c["id"] == bug_id.as_str()).unwrap();
    assert_eq!(bug2["title"], "Login returns 500 (updated)", "the field refreshed");
    assert_eq!(bug2["lane"], "performing", "the LOCAL lane move is respected on re-import");

    // Dismiss the feature card (move to done); a re-import must not refile it.
    request(&mut ws, &Envelope::message("dis", "card.move", serde_json::json!({ "card_id": feat_id, "column": "done" })), &mut sink).await;
    let imp3 = request(&mut ws, &Envelope::message("im3", "github.issues.import", serde_json::json!({ "project_id": project_id })), &mut sink).await;
    let results3 = imp3.payload["results"].as_array().unwrap();
    let feat_res = results3.iter().find(|r| r["number"] == 15).unwrap();
    assert_eq!(feat_res["outcome"], "suppressed", "a dismissed card is not refiled: {results3:?}");

    // Preview now reflects dedupe status.
    let preview2 = request(&mut ws, &Envelope::message("pv2", "github.issues.preview", serde_json::json!({ "project_id": project_id })), &mut sink).await;
    let pv2 = preview2.payload["issues"].as_array().unwrap();
    assert_eq!(pv2.iter().find(|i| i["number"] == 12).unwrap()["dedupe"], "tracked");
    assert_eq!(pv2.iter().find(|i| i["number"] == 15).unwrap()["dedupe"], "dismissed");
}

#[tokio::test]
async fn absent_gh_degrades_cleanly() {
    // Point DFLOW_GH at a program that does not exist: the transport reports gh absent
    // and import fails with the setup pointer, not a panic (`gate.md`: absent-gh degrades).
    let data_dir = unique_data_dir("ghabsent");
    let repo = scratch_repo(&data_dir);
    let env = vec![("DFLOW_GH", "definitely-not-a-real-gh-binary-xyz")];
    let (_daemon, port, token) = start_daemon(&data_dir, &env);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let auth = request(&mut ws, &Envelope::message("a", "github.auth.status", serde_json::json!({})), &mut sink).await;
    assert_eq!(auth.payload["present"], false, "a missing gh reports not present");
    assert_eq!(auth.payload["authenticated"], false);

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let imp = request(&mut ws, &Envelope::message("im", "github.issues.import", serde_json::json!({ "project_id": project_id })), &mut sink).await;
    assert_eq!(imp.msg_type, "error", "import without gh must error cleanly: {imp:?}");
    assert!(
        imp.payload["message"].as_str().unwrap_or("").contains("gh"),
        "the error points at gh setup: {imp:?}"
    );
}
