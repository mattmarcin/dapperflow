//! Ship-path e2e (`gate.md` / Ship, Teardown safety; `roadmap.md` M5.4). Proves the full
//! M5 acceptance chain end to end with no live GitHub: a GitHub issue is imported to a
//! card, an agent commits work, the gate passes, the branch is pushed to a real bare git
//! remote through the git CLI, a PR is opened via a stub gh with a generated `Fixes #<n>`
//! body, CI is watched, the PR is squash-merged, and the worktree returns only after the
//! landed-work proof passes.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::Envelope;

fn dflow_binary() -> PathBuf {
    let dflowd = PathBuf::from(env!("CARGO_BIN_EXE_dflowd"));
    let name = if cfg!(windows) { "dflow.exe" } else { "dflow" };
    dflowd.parent().unwrap().join(name)
}

fn stub_gh() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stub-gh"))
}

fn ps_launch(script: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(script);
    serde_json::json!([
        "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", path.to_string_lossy()
    ])
    .to_string()
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn wait_for_file(dir: &str, name: &str, timeout: Duration) -> bool {
    let path = PathBuf::from(dir).join(name);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

async fn poll_gate_status(ws: &mut Ws, card_id: &str, timeout: Duration) -> serde_json::Value {
    let mut sink = Vec::new();
    let deadline = Instant::now() + timeout;
    loop {
        let st = request(ws, &Envelope::message("gs", "gate.status", serde_json::json!({ "card_id": card_id })), &mut sink).await;
        let status = st.payload["run"]["status"].as_str().unwrap_or("");
        if matches!(status, "passed" | "failed" | "escalated") {
            return st.payload;
        }
        assert!(Instant::now() < deadline, "gate never settled: {:?}", st.payload);
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

#[tokio::test]
async fn issue_to_pr_to_merge_ship_path() {
    let dflow = dflow_binary();
    let gh = stub_gh();
    if !dflow.exists() || !gh.exists() {
        eprintln!("SKIP: dflow/stub-gh not built; run under `cargo test --workspace`");
        return;
    }
    let data_dir = unique_data_dir("ship");
    let repo = scratch_repo(&data_dir);

    // A real bare remote so the push goes through the git CLI for real (no creds needed).
    let origin = data_dir.join("origin.git");
    git(&data_dir, &["init", "--bare", &origin.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", &origin.to_string_lossy()]);
    git(&repo, &["push", "origin", "main"]);

    // The stub gh serves one open issue (#7) and stores PR state across invocations.
    let issues_path = data_dir.join("issues.json");
    std::fs::write(&issues_path, serde_json::json!([
        { "number": 7, "title": "Fix the widget crash", "body": "The widget crashes on empty input.",
          "state": "OPEN", "url": "https://github.com/acme/web/issues/7",
          "labels": [{"name": "bug"}], "assignees": [], "milestone": null }
    ]).to_string()).unwrap();
    let gh_state = data_dir.join("ghstate");

    let gh_str = gh.to_string_lossy().into_owned();
    let issues_str = issues_path.to_string_lossy().into_owned();
    let state_str = gh_state.to_string_lossy().into_owned();
    let author = ps_launch("stub_gate_author.ps1");
    let (_daemon, port, token) = start_daemon(
        &data_dir,
        &[
            ("DFLOW_GH", gh_str.as_str()),
            ("DFLOW_STUB_GH_AUTHED", "1"),
            ("DFLOW_STUB_GH_REPO", "acme/web/main"),
            ("DFLOW_STUB_GH_ISSUES", issues_str.as_str()),
            ("DFLOW_STUB_GH_STATE", state_str.as_str()),
            ("DFLOW_LAUNCH_STUB", author.as_str()),
        ],
    );
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Project + a passing check that runs against the checked-out code.
    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    request(&mut ws, &Envelope::message("pu", "project.update", serde_json::json!({ "project_id": project_id, "check_cmds": [{ "name": "smoke", "cmd": "findstr /C:function feature.txt" }] })), &mut sink).await;

    // Import the issue -> an origin card on the checks_only `standard` recipe.
    let imp = request(&mut ws, &Envelope::message("im", "github.issues.import", serde_json::json!({ "project_id": project_id, "dial_recipe": "standard" })), &mut sink).await;
    let card_id = imp.payload["results"][0]["card_id"].as_str().unwrap().to_string();

    // The agent (author stub) commits the fix in its worktree.
    let disp = request(&mut ws, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let author_wt = disp.payload["worktree_path"].as_str().unwrap().to_string();
    assert!(wait_for_file(&author_wt, "author.log", Duration::from_secs(30)), "author never committed");

    // The gate passes (checks_only, green check) and keeps its worktree leased for ship.
    let gr = request(&mut ws, &Envelope::message("g", "gate.run", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    assert_eq!(gr.payload["strictness"], "checks_only");
    let status = poll_gate_status(&mut ws, &card_id, Duration::from_secs(60)).await;
    assert_eq!(status["run"]["status"], "passed", "the gate passes: {status:?}");

    // Ship: push the branch to the real bare remote + open a PR via gh with Fixes #7.
    let ship = request(&mut ws, &Envelope::message("sh", "gate.ship", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    assert_eq!(ship.msg_type, "gate.ship", "ship failed: {ship:?}");
    assert_eq!(ship.payload["mode"], "pr");
    assert_eq!(ship.payload["pushed"], true, "the branch was pushed: {ship:?}");
    let pr_number = ship.payload["pr_number"].as_i64().unwrap();

    // The branch really reached the bare remote through the git CLI.
    let remote_branches = git(&origin, &["branch", "--list", "dapperflow/gate/*"]);
    assert!(remote_branches.contains("dapperflow/gate/"), "the gate branch is on the remote: {remote_branches:?}");

    // The generated PR body carried `Fixes #7`, so GitHub closes the issue on merge.
    let body = std::fs::read_to_string(gh_state.join("pr_create_body.txt")).unwrap();
    assert!(body.contains("Fixes #7"), "the PR body links the issue for auto-close: {body:?}");
    assert!(body.contains(&card_id), "the PR body links the card: {body:?}");

    // Merge (squash default), watch CI, and prove landing before teardown.
    let merge = request(&mut ws, &Envelope::message("mg", "gate.merge", serde_json::json!({ "card_id": card_id })), &mut sink).await;
    assert_eq!(merge.msg_type, "gate.merge", "merge failed: {merge:?}");
    assert_eq!(merge.payload["merged"], true, "the PR merged: {merge:?}");
    assert_eq!(merge.payload["landed"], true, "the landed-work proof passed: {merge:?}");

    // The merge used squash by default.
    let merge_args = std::fs::read_to_string(gh_state.join("pr_merge_args.txt")).unwrap();
    assert!(merge_args.contains("--squash"), "squash is the default merge method: {merge_args:?}");

    // The delivery timeline is complete.
    let cget = request(&mut ws, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 400 })), &mut sink).await;
    let events = cget.payload["events"].as_array().unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap_or("")).collect();
    for want in ["pushed", "pr_opened", "ci_status", "merged", "worktree_returned"] {
        assert!(kinds.contains(&want), "missing {want} event; kinds={kinds:?}");
    }
    let pr_opened = events.iter().find(|e| e["kind"] == "pr_opened").unwrap();
    assert_eq!(pr_opened["payload"]["fixes"], 7, "the PR records the fixed issue number");
    assert_eq!(pr_opened["payload"]["pr_number"], pr_number);
    let returned = events.iter().find(|e| e["kind"] == "worktree_returned").unwrap();
    assert_eq!(returned["payload"]["outcome"], "clean", "the worktree returned clean after a proven landing");
}
