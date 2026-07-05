//! Live proof (opt-in, `#[ignore]`) for audit finding #4: a real `claude --model haiku`
//! planning session that HOLDS the plan-review loop instead of backgrounding the poll and
//! ending early (`plan-studio.md`, `docs/spikes/fix-plan-loop.md`).
//!
//! Run with:
//!   cargo build -p dflow-cli && \
//!   cargo test -p dflowd --test live_plan_loop -- --ignored --nocapture
//!
//! Requires the `claude` CLI on PATH and real credits. It dispatches a haiku session on
//! the `deep` recipe (plan stage, approval required), so the composed brief carries the
//! injected plan-review-loop protocol - the exact guidance this fix adds. The human side
//! (this test) reacts to plan-round events: it submits feedback on round 1 and approves on
//! round 2, asserting the session parks in `awaiting_feedback` while the agent holds a
//! FOREGROUND `dflow plan poll`, and that the loop reaches `approved`. DFLOW_DATA_DIR
//! isolation means it never touches a live user daemon.

mod common;

use std::time::{Duration, Instant};

use base64::Engine;
use common::*;
use dflow_proto::Envelope;

/// The planning subject. The mechanics (write plan.html, open, hold a foreground poll,
/// revise + re-open until approved) come from the recipe's injected loop protocol, not
/// from this brief - that is what makes this a clean proof of the guidance fix.
const BRIEF: &str = "\
Plan a retry policy for our HTTP API client: decide the maximum number of attempts, the \
backoff strategy, and what to do on final failure. Keep it to a one-screen, self-contained \
plan.html. Then follow the plan review loop exactly as described in your DapperFlow \
instructions - open it, hold a foreground `dflow plan poll`, and revise + re-open on \
feedback - until the plan is approved. Do not start implementation until it is approved.";

fn claude_on_path() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for name in ["claude", "claude.exe", "claude.cmd", "claude.bat"] {
            if dir.join(name).exists() {
                return true;
            }
        }
    }
    false
}

#[tokio::test]
#[ignore = "live: launches a real claude planning session on haiku; run with --ignored"]
async fn live_haiku_holds_the_plan_loop() {
    if !claude_on_path() {
        eprintln!("SKIP: claude not on PATH");
        return;
    }

    let data_dir = unique_data_dir("live-planloop");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "info")]);

    let mut ctrl = connect_and_auth(port, &token).await;
    let mut events = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(
        &mut ctrl,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    let cadd = request(
        &mut ctrl,
        &Envelope::message(
            "c",
            "card.create",
            serde_json::json!({ "title": "retry policy plan (live haiku)", "type": "feature", "project_id": project_id, "brief": BRIEF }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // A launcher that skips permission prompts so haiku can run `dflow` unattended.
    let _ = request(
        &mut ctrl,
        &Envelope::message(
            "ag",
            "agents.add",
            serde_json::json!({ "name": "haiku-yolo", "adapter": "claude", "command": "claude", "extra_args": ["--dangerously-skip-permissions"] }),
        ),
        &mut sink,
    )
    .await;

    // Subscribe before dispatch so no plan-round event is missed.
    let _ = request(&mut events, &Envelope::message("sub", "event.subscribe", serde_json::json!({})), &mut sink).await;

    // Dispatch a real haiku session on the `deep` recipe (plan stage, approval required),
    // so the injected plan-review-loop protocol rides the composed brief.
    let disp = request(
        &mut ctrl,
        &Envelope::message(
            "d",
            "dispatch.start",
            serde_json::json!({ "card_id": card_id, "agent": "haiku-yolo", "model": "haiku", "recipe": "deep" }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(disp.msg_type, "dispatch.start", "dispatch failed: {disp:?}");
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();
    let worktree_path = disp.payload["worktree_path"].as_str().unwrap_or("").to_string();
    println!("dispatched live haiku session {session_id} for card {card_id}");
    println!("worktree: {worktree_path}");

    // Drive the human side reactively: round 1 -> feedback, round >= 2 -> approve. Assert
    // the session parks in awaiting_feedback while the agent holds the foreground poll.
    let mut artifact_id = String::new();
    let mut saw_awaiting = false;
    let mut approved = false;
    let mut rounds_seen: Vec<u64> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(600);
    while Instant::now() < deadline && !approved {
        let ev = match tokio::time::timeout(Duration::from_secs(300), next_envelope(&mut events)).await {
            Ok(ev) => ev,
            Err(_) => {
                println!("timed out waiting for a plan event; last state:");
                break;
            }
        };
        if ev.msg_type != "event.card_event" {
            continue;
        }
        let e = &ev.payload["event"];
        let kind = e["kind"].as_str().unwrap_or("");
        println!("event: {kind}  payload={}", e["payload"]);
        match kind {
            "artifact_opened" => {
                artifact_id = e["payload"]["artifact_id"].as_str().unwrap_or("").to_string();
            }
            "plan_round" => {
                let round = e["payload"]["round"].as_u64().unwrap_or(0);
                if artifact_id.is_empty() {
                    artifact_id = e["payload"]["artifact_id"].as_str().unwrap_or("").to_string();
                }
                if rounds_seen.contains(&round) {
                    continue;
                }
                rounds_seen.push(round);
                if round <= 1 {
                    // The agent opened and is polling: it must park in awaiting_feedback.
                    let parked = wait_for_state(&mut ctrl, &session_id, "awaiting_feedback", Duration::from_secs(120)).await;
                    saw_awaiting |= parked;
                    println!("round {round}: session parked in awaiting_feedback = {parked}; submitting feedback");
                    submit_feedback(&mut ctrl, &artifact_id, round).await;
                } else {
                    println!("round {round}: revised artifact re-opened; approving");
                    submit_approve(&mut ctrl, &artifact_id, round).await;
                }
            }
            "plan_approved" => approved = true,
            _ => {}
        }
    }

    // Capture the session transcript (proof the real CLI ran open/poll in the foreground).
    let transcript = attach_scrollback(&mut ctrl, &session_id).await;
    println!("---- session transcript (tail) ----");
    for line in transcript.lines().rev().take(60).collect::<Vec<_>>().into_iter().rev() {
        let t = line.trim_end();
        if !t.is_empty() {
            println!("{t}");
        }
    }

    // Final on-card evidence.
    let cget = request(&mut ctrl, &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id, "events_limit": 200 })), &mut sink).await;
    let kinds: Vec<String> = cget.payload["events"].as_array().unwrap().iter().filter_map(|e| e["kind"].as_str().map(String::from)).collect();
    let artifacts = cget.payload["artifacts"].as_array().cloned().unwrap_or_default();
    println!("---- live evidence ----");
    println!("rounds seen: {rounds_seen:?}");
    println!("saw awaiting_feedback while polling: {saw_awaiting}");
    println!("card events: {kinds:?}");
    if let Some(a) = artifacts.first() {
        println!("artifact status: {}  round: {}", a["status"], a["round"]);
    }

    // Stop the session (it may otherwise proceed into implementation).
    let _ = request(&mut ctrl, &Envelope::message("cancel", "dispatch.cancel", serde_json::json!({ "card_id": card_id })), &mut sink).await;

    // The foreground poll being held is proven by `saw_awaiting`: `awaiting_feedback` is
    // set only by the `dflow plan poll` handler (api.rs), so parking there means the agent
    // ran the poll in the foreground and blocked on it rather than backgrounding it.
    assert!(saw_awaiting, "the session must park in awaiting_feedback while the agent holds the foreground poll");
    assert!(approved, "the plan loop must reach approval; rounds seen: {rounds_seen:?}");
    assert!(rounds_seen.iter().any(|r| *r >= 2), "the agent must revise and re-open at least once: {rounds_seen:?}");
    assert_eq!(artifacts.first().map(|a| a["status"].clone()), Some(serde_json::json!("approved")));
    // Corroborate that the real CLI drove the loop. The scrollback snapshot is a moving
    // window, so assert on `dflow plan` (open/poll) generally rather than a specific verb
    // that may have scrolled off; the event timeline above is the authoritative proof.
    assert!(transcript.contains("dflow plan"), "the transcript should show the real dflow plan CLI driving the loop");
    println!("LIVE PROOF PASSED: real haiku held the plan loop through feedback, re-open, and approval.");
}

/// Poll session.list until the session reaches `want`, or timeout.
async fn wait_for_state(ctrl: &mut Ws, session_id: &str, want: &str, timeout: Duration) -> bool {
    let mut sink = Vec::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let listing = request(ctrl, &Envelope::message("l", "session.list", serde_json::json!({})), &mut sink).await;
        if let Some(row) = listing.payload["sessions"].as_array().and_then(|a| a.iter().find(|s| s["session_id"] == session_id)) {
            if row["state"] == want {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    false
}

async fn submit_feedback(ctrl: &mut Ws, artifact_id: &str, round: u64) {
    let mut sink = Vec::new();
    let r = request(
        ctrl,
        &Envelope::message(
            "sub",
            "artifact.feedback.submit",
            serde_json::json!({
                "artifact_id": artifact_id,
                "round": round,
                "items": [
                    { "kind": "chat", "body": "Cap retries at 3 attempts, then dead-letter instead of retrying forever." },
                    { "kind": "control", "question_key": "backoff", "value": "exponential-with-jitter" }
                ]
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(r.payload["ok"], true, "feedback submit failed: {r:?}");
}

async fn submit_approve(ctrl: &mut Ws, artifact_id: &str, round: u64) {
    let mut sink = Vec::new();
    let r = request(
        ctrl,
        &Envelope::message(
            "approve",
            "artifact.feedback.submit",
            serde_json::json!({
                "artifact_id": artifact_id,
                "round": round,
                "items": [ { "kind": "action", "action": "approve_plan" } ]
            }),
        ),
        &mut sink,
    )
    .await;
    assert_eq!(r.payload["ok"], true, "approve failed: {r:?}");
}

/// Attach to the session and return its decoded scrollback (for the transcript).
async fn attach_scrollback(ctrl: &mut Ws, session_id: &str) -> String {
    let mut sink = Vec::new();
    let attached = request(
        ctrl,
        &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 140, "rows": 48 })),
        &mut sink,
    )
    .await;
    let b64 = attached.payload["replay_base64"].as_str().unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}
