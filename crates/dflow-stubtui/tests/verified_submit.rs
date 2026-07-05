//! The verified-submit test matrix (`adapters.md` / Verified submit): drive the stub
//! TUI through a real PTY and confirm the algorithm submits despite popup swallow,
//! placeholder expansion, ghost text, and slow redraw - and correctly reports failure
//! when submission is impossible.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use dflow_core::{bundled_manifests, send_verified, SessionManager, SessionSpec, SubmitConfig};

/// Spawn the stub TUI in `mode` and run verified submit of `text` against it.
fn run(mode: &str, text: &str) -> dflow_core::VerifiedSubmit {
    let stub = env!("CARGO_BIN_EXE_dflow-stubtui");
    let mut env = BTreeMap::new();
    env.insert("DFLOW_STUB_MODE".to_string(), mode.to_string());

    let mgr = SessionManager::new();
    let session = mgr
        .create(SessionSpec {
            harness: "claude".into(),
            command: vec![stub.to_string()],
            cols: 80,
            rows: 24,
            env,
            ..Default::default()
        })
        .expect("spawn stub tui");

    // Wait for the stub to draw its initial composer prompt.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !session.capture_plain().contains('>') {
        std::thread::sleep(Duration::from_millis(50));
    }

    let manifest = bundled_manifests().get("claude").unwrap();
    // Fast timings for the test; production values come from SubmitConfig::from_manifest.
    let cfg = SubmitConfig {
        max_attempts: 6,
        redraw_wait: Duration::from_millis(200),
        popup_settle: Duration::from_millis(250),
        enter_bytes: b"\r".to_vec(),
    };
    let outcome = send_verified(&session, manifest, text, &cfg);
    session.kill();
    outcome
}

#[test]
fn normal_submit_first_try() {
    let out = run("normal", "fix the login bug");
    assert!(out.submitted, "normal composer should submit");
    assert_eq!(out.attempts, 1, "no retries needed on a clean composer");
}

#[test]
fn popup_swallow_recovers_on_retry() {
    // The first Enter on a slash command is swallowed by the popup; verified submit
    // retries and the second Enter lands.
    let out = run("popup_swallow", "/deploy");
    assert!(out.submitted, "verified submit should recover from a swallowed Enter");
    assert!(out.attempts >= 2, "the swallowed first Enter forces a retry, got {}", out.attempts);
}

#[test]
fn placeholder_expansion_recovers_on_retry() {
    // The first Enter expands an argument-hint placeholder instead of submitting.
    let out = run("placeholder", "review");
    assert!(out.submitted, "verified submit should recover from placeholder expansion");
    assert!(out.attempts >= 2, "expansion forces a retry, got {}", out.attempts);
}

#[test]
fn ghost_text_is_not_mistaken_for_input() {
    // Dim ghost text is drawn beside the composer; verified submit still submits.
    let out = run("ghost", "hello there");
    assert!(out.submitted, "ghost text must not block a clean submit");
}

#[test]
fn slow_redraw_still_verifies() {
    // The redraw after Enter lags; verified submit's re-read + retry tolerates it.
    let out = run("slow", "compile it");
    assert!(out.submitted, "verified submit should tolerate a slow redraw");
}

#[test]
fn impossible_submit_reports_failure() {
    // The composer never clears: verified submit exhausts its attempts and reports
    // failure so the caller raises Needs You instead of silently dropping the message.
    let out = run("never", "this will not send");
    assert!(!out.submitted, "an unsubmittable composer must report failure");
    assert_eq!(out.attempts, 6, "failure only after the bounded attempts are spent");
}
