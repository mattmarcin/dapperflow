//! Gate engine e2e against a real daemon with STUB harnesses (`gate.md` / Pipeline,
//! `roadmap.md` M5.3). No real LLM: an author stub commits a seeded bug, a reviewer stub
//! (on a DIFFERENT harness than the author) files findings via the real `dflow finding
//! add` CLI, and a fixer stub applies the safe-mechanical one. This proves the full
//! checks -> review -> autofix -> escalate flow, the events, and the reviewer-differs
//! enforcement.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::Envelope;

fn dflow_binary() -> PathBuf {
    let dflowd = PathBuf::from(env!("CARGO_BIN_EXE_dflowd"));
    let name = if cfg!(windows) { "dflow.exe" } else { "dflow" };
    dflowd.parent().unwrap().join(name)
}

fn ps_launch(script: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(script);
    serde_json::json!([
        "powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", path.to_string_lossy()
    ])
    .to_string()
}

/// Write a project-scope `gate: full` recipe into the repo so the gate runs the full
/// pipeline (the bundled recipes ship checks_only; the engine now honors the recipe's
/// declared strictness - `roadmap.md` M5.5 / interim-behavior upgrade).
fn write_full_recipe(repo: &std::path::Path) {
    let dir = repo.join(".dapperflow").join("recipes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("gatefull.md"),
        "---\nname: gatefull\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: full\n  reviewer_harness: different\nship:\n  target: pr\n---\n\n## verify\nRun the full gate.\n",
    )
    .unwrap();
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
        assert!(Instant::now() < deadline, "gate never reached a terminal status: {:?}", st.payload);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Wait until a file appears in a worktree (the author/fixer stubs write a marker on done).
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

#[tokio::test]
async fn gate_full_pipeline_checks_review_autofix_escalate() {
    let dflow = dflow_binary();
    if !dflow.exists() {
        eprintln!("SKIP: dflow CLI not built next to dflowd ({}); build with `cargo test --workspace`", dflow.display());
        return;
    }
    let author = ps_launch("stub_gate_author.ps1");
    let reviewer = ps_launch("stub_gate_reviewer.ps1");
    let fixer = ps_launch("stub_gate_fixer.ps1");

    let data_dir = unique_data_dir("gatefull");
    let repo = scratch_repo(&data_dir);
    write_full_recipe(&repo);
    let (_daemon, port, token) = start_daemon(
        &data_dir,
        &[
            ("DFLOW_LAUNCH_STUB", author.as_str()),
            ("DFLOW_LAUNCH_REVIEWERSTUB", reviewer.as_str()),
            ("DFLOW_LAUNCH_FIXERSTUB", fixer.as_str()),
            ("DFLOW_GATE_FIXER_HARNESS", "fixerstub"),
            ("DFLOW_GATE_SESSION_TIMEOUT_MS", "45000"),
        ],
    );
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Project + a passing check that runs against the checked-out code.
    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    request(&mut ws, &Envelope::message("pu", "project.update", serde_json::json!({ "project_id": project_id, "check_cmds": [{ "name": "smoke", "cmd": "findstr /C:function feature.txt" }] })), &mut sink).await;

    // A card on the full-gate recipe.
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", serde_json::json!({ "title": "add feature", "type": "feature", "project_id": project_id, "dial_recipe": "gatefull" })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Dispatch the author stub; wait for its commit.
    let disp = request(&mut ws, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    assert_eq!(disp.msg_type, "dispatch.start", "author dispatch failed: {disp:?}");
    let author_wt = disp.payload["worktree_path"].as_str().unwrap().to_string();
    assert!(wait_for_file(&author_wt, "author.log", Duration::from_secs(30)), "author stub never committed");

    // Run the gate: the reviewer is on a DIFFERENT harness than the author (stub).
    let gr = request(&mut ws, &Envelope::message("g", "gate.run", serde_json::json!({ "card_id": card_id, "reviewer_harness": "reviewerstub" })), &mut sink).await;
    assert_eq!(gr.msg_type, "gate.run", "gate.run failed: {gr:?}");
    assert_eq!(gr.payload["strictness"], "full", "the engine honored the recipe's gate: full");

    // The pipeline runs asynchronously; wait for a terminal status.
    let status = poll_gate_status(&mut ws, &card_id, Duration::from_secs(120)).await;
    assert_eq!(status["run"]["status"], "escalated", "an intent finding escalates: {status:?}");
    assert_eq!(status["run"]["reviewer_harness"], "reviewerstub");
    assert_eq!(status["run"]["author_harness"], "stub");

    // Findings: two filed; the mechanical one autofixed, the intent one still open.
    let findings = status["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "reviewer filed two findings: {findings:?}");
    let mechanical = findings.iter().find(|f| f["category"] == "mechanical").unwrap();
    let intent = findings.iter().find(|f| f["category"] == "intent").unwrap();
    assert_eq!(mechanical["resolution"], "autofixed", "the mechanical finding was autofixed");
    assert!(intent["resolution"].is_null(), "the intent finding stays open for the human");
    assert!(
        intent["body"].as_str().unwrap().contains("off-by-one"),
        "the reviewer caught the seeded bug: {intent:?}"
    );
    let intent_id = intent["id"].as_str().unwrap().to_string();

    // The timeline carries the gate events with evidence.
    let cget = request(&mut ws, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 300 })), &mut sink).await;
    let events = cget.payload["events"].as_array().unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap_or("")).collect();
    for want in ["gate_started", "gate_step", "finding_raised", "finding_resolved", "gate_failed"] {
        assert!(kinds.contains(&want), "missing {want} event; kinds={kinds:?}");
    }
    // The checks step carries an evidence pointer, never a prose-only claim.
    let checks_step = events.iter().find(|e| e["kind"] == "gate_step" && e["payload"]["step"] == "checks").unwrap();
    assert!(checks_step["payload"]["evidence"]["log"].is_string(), "checks evidence points at a log: {checks_step:?}");

    // The escalation raised a gate_finding Needs You item.
    let fleet = request(&mut ws, &Envelope::message("f", "fleet.status", serde_json::json!({})), &mut sink).await;
    let needs = fleet.payload["needs_you"].as_array().unwrap();
    assert!(needs.iter().any(|n| n["kind"] == "gate_finding" && n["card_id"] == card_id), "gate_finding Needs You raised: {needs:?}");

    // The human resolves the intent finding in chrome; the Needs You clears.
    let resolve = request(&mut ws, &Envelope::message("rf", "gate.resolve_finding", serde_json::json!({ "finding_id": intent_id, "resolution": "accepted" })), &mut sink).await;
    assert_eq!(resolve.msg_type, "gate.resolve_finding", "resolve failed: {resolve:?}");
    assert_eq!(resolve.payload["finding"]["resolution"], "accepted");

    let fleet2 = request(&mut ws, &Envelope::message("f2", "fleet.status", serde_json::json!({})), &mut sink).await;
    let needs2 = fleet2.payload["needs_you"].as_array().unwrap();
    assert!(!needs2.iter().any(|n| n["kind"] == "gate_finding" && n["card_id"] == card_id), "resolving the last finding clears the Needs You: {needs2:?}");
}

#[tokio::test]
async fn gate_full_passes_when_only_mechanical_findings() {
    // The full-gate PASS path: the reviewer files only a safe-mechanical finding, the
    // fixer applies it, the re-check stays green, and with no open findings the gate
    // passes (checks -> review -> autofix -> pass). No LLM.
    let dflow = dflow_binary();
    if !dflow.exists() {
        eprintln!("SKIP: dflow CLI not built");
        return;
    }
    let author = ps_launch("stub_gate_author.ps1");
    let reviewer = ps_launch("stub_gate_reviewer_mech.ps1");
    let fixer = ps_launch("stub_gate_fixer.ps1");

    let data_dir = unique_data_dir("gatepass");
    let repo = scratch_repo(&data_dir);
    write_full_recipe(&repo);
    let (_daemon, port, token) = start_daemon(
        &data_dir,
        &[
            ("DFLOW_LAUNCH_STUB", author.as_str()),
            ("DFLOW_LAUNCH_REVIEWERSTUB", reviewer.as_str()),
            ("DFLOW_LAUNCH_FIXERSTUB", fixer.as_str()),
            ("DFLOW_GATE_FIXER_HARNESS", "fixerstub"),
            ("DFLOW_GATE_SESSION_TIMEOUT_MS", "45000"),
        ],
    );
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    request(&mut ws, &Envelope::message("pu", "project.update", serde_json::json!({ "project_id": project_id, "check_cmds": [{ "name": "smoke", "cmd": "findstr /C:function feature.txt" }] })), &mut sink).await;
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", serde_json::json!({ "title": "clean feature", "type": "feature", "project_id": project_id, "dial_recipe": "gatefull" })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    let disp = request(&mut ws, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let author_wt = disp.payload["worktree_path"].as_str().unwrap().to_string();
    assert!(wait_for_file(&author_wt, "author.log", Duration::from_secs(30)), "author never committed");

    request(&mut ws, &Envelope::message("g", "gate.run", serde_json::json!({ "card_id": card_id, "reviewer_harness": "reviewerstub" })), &mut sink).await;
    let status = poll_gate_status(&mut ws, &card_id, Duration::from_secs(120)).await;
    assert_eq!(status["run"]["status"], "passed", "an all-mechanical gate passes after autofix: {status:?}");
    let findings = status["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "one mechanical finding: {findings:?}");
    assert_eq!(findings[0]["resolution"], "autofixed");

    // No escalation Needs You when everything autofixed.
    let fleet = request(&mut ws, &Envelope::message("f", "fleet.status", serde_json::json!({})), &mut sink).await;
    let needs = fleet.payload["needs_you"].as_array().unwrap();
    assert!(!needs.iter().any(|n| n["kind"] == "gate_finding" && n["card_id"] == card_id), "a clean autofix raises no gate_finding: {needs:?}");

    // A gate_passed event is on the timeline.
    let cget = request(&mut ws, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 300 })), &mut sink).await;
    let kinds: Vec<&str> = cget.payload["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap_or("")).collect();
    assert!(kinds.contains(&"gate_passed"), "gate_passed recorded: {kinds:?}");
}

#[tokio::test]
async fn gate_refuses_reviewer_on_the_same_harness_as_author() {
    let data_dir = unique_data_dir("gatediff");
    let repo = scratch_repo(&data_dir);
    write_full_recipe(&repo);
    // A no-op author launch (the commit is seeded directly below so no CLI is needed).
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", serde_json::json!({ "title": "same harness", "type": "feature", "project_id": project_id, "dial_recipe": "gatefull" })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Seed a commit to gate (HEAD of main), and force author == reviewer harness.
    let head = git_head(&repo);
    let gr = request(&mut ws, &Envelope::message("g", "gate.run", serde_json::json!({ "card_id": card_id, "head_sha": head, "author_harness": "claude", "reviewer_harness": "claude" })), &mut sink).await;
    assert_eq!(gr.msg_type, "gate.run", "gate.run should start: {gr:?}");

    let status = poll_gate_status(&mut ws, &card_id, Duration::from_secs(30)).await;
    assert_eq!(status["run"]["status"], "failed", "an equal author/reviewer harness must fail the gate: {status:?}");
    let cget = request(&mut ws, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 200 })), &mut sink).await;
    let events = cget.payload["events"].as_array().unwrap();
    let review = events.iter().find(|e| e["kind"] == "gate_step" && e["payload"]["step"] == "review");
    assert!(
        review.map(|r| r["payload"]["evidence"]["reason"].as_str().unwrap_or("").contains("must differ")).unwrap_or(false),
        "the review step explains the reviewer must differ: {events:?}"
    );
}

#[tokio::test]
async fn gate_checks_only_fails_on_a_red_check() {
    let data_dir = unique_data_dir("gatechecks");
    let repo = scratch_repo(&data_dir);
    // Default (checks_only) bundled behavior via the standard recipe; a red check fails.
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut ws, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    request(&mut ws, &Envelope::message("pu", "project.update", serde_json::json!({ "project_id": project_id, "check_cmds": [{ "name": "always_red", "cmd": "exit 1" }] })), &mut sink).await;
    let cadd = request(&mut ws, &Envelope::message("c", "card.create", serde_json::json!({ "title": "red", "type": "bug", "project_id": project_id, "dial_recipe": "standard" })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    let head = git_head(&repo);
    let gr = request(&mut ws, &Envelope::message("g", "gate.run", serde_json::json!({ "card_id": card_id, "head_sha": head, "author_harness": "claude" })), &mut sink).await;
    assert_eq!(gr.payload["strictness"], "checks_only", "the standard recipe is checks_only");

    let status = poll_gate_status(&mut ws, &card_id, Duration::from_secs(40)).await;
    assert_eq!(status["run"]["status"], "failed", "a red check fails the gate: {status:?}");
    // The failed check is recorded as a blocker finding with an evidence pointer.
    let findings = status["findings"].as_array().unwrap();
    let check_finding = findings.iter().find(|f| f["source"] == "check").unwrap();
    assert_eq!(check_finding["severity"], "blocker");
    assert!(check_finding["evidence"].is_string(), "the check finding points at its log");
}

/// The HEAD commit sha of a repo (`git rev-parse HEAD`).
fn git_head(repo: &std::path::Path) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
