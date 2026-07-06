//! Shim-launch brief delivery (`adapters.md` / Dispatch brief delivery): prove that a
//! multi-line dispatch brief, delivered by TYPED injection to a harness launched through a
//! `cmd.exe /c` shim, reaches the agent's composer INTACT - the whole brief, not just the
//! first line.
//!
//! This is the deterministic reproduction of the fix. The bug: a shim harness
//! (codex/opencode/pi install as a `*.cmd` next to a `#!/bin/sh` shim) launches under
//! `cmd.exe /c`, and `cmd.exe` truncates any multi-line argument at the first newline, so a
//! launch-argument brief arrived as only the card title. The fix delivers the brief by the
//! readiness-gated verified-submit path AFTER launch, so every line lands.

#![cfg(windows)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dflow_core::agents::launchable_command;
use dflow_core::steer::wait_for_composer_ready;
use dflow_core::{bundled_manifests, send_verified, SessionManager, SessionSpec, SubmitConfig};

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-brief-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn shim_launch_delivers_full_multiline_brief_typed() {
    let dir = unique_dir("shim");
    let stub = env!("CARGO_BIN_EXE_dflow-stubtui");
    let capture = dir.join("captured.txt");

    // A `.cmd` shim that launches the stub TUI - the exact finding #2 shape (a real
    // codex/opencode/pi launch is `codex.cmd` -> node). `launchable_command` must rewrite
    // it to run under `cmd.exe /c`; that is the launch form `cmd.exe` would truncate a
    // multi-line argument on.
    let shim = dir.join("agent.cmd");
    std::fs::write(&shim, format!("@echo off\r\n\"{stub}\"\r\n")).unwrap();
    let shim_argv = vec![shim.to_string_lossy().into_owned()];
    let launchable = launchable_command(&shim_argv);
    assert_eq!(
        launchable.first().map(String::as_str),
        Some("cmd.exe"),
        "the .cmd shim must resolve to a cmd.exe launch (the truncation path): {launchable:?}"
    );

    // The composed brief is multi-line: the card title is line 1, and load-bearing detail
    // (acceptance criteria, the dflow contract) sits BELOW the first newline - exactly what
    // `cmd.exe` truncation used to drop.
    let brief = "CARD TITLE alpha-one\n\
                 standing guidance line beta-two\n\
                 recipe stage line gamma-three\n\
                 Acceptance criteria: reply with the word quokka\n\
                 dflow usage contract line omega-last";

    let mut env = BTreeMap::new();
    env.insert("DFLOW_STUB_CAPTURE".to_string(), capture.to_string_lossy().into_owned());

    let mgr = SessionManager::new();
    let session = mgr
        .create(SessionSpec {
            // opencode's manifest declares typed delivery / submit=enter / paste=none - the
            // exact production input contract for a shim harness.
            harness: "opencode".into(),
            command: shim_argv,
            cols: 100,
            rows: 30,
            env,
            ..Default::default()
        })
        .expect("spawn shim -> stub tui");

    let manifest = bundled_manifests().get("opencode").unwrap();
    assert!(
        wait_for_composer_ready(&session, manifest, Duration::from_secs(15)),
        "the stub composer never became ready"
    );

    let cfg = SubmitConfig {
        max_attempts: 6,
        redraw_wait: Duration::from_millis(200),
        popup_settle: Duration::from_millis(250),
        enter_bytes: manifest.composer.submit_bytes(),
    };
    let outcome = send_verified(&session, manifest, brief, &cfg);
    assert!(outcome.submitted, "verified submit must report the brief landed (submitted:true)");

    // The full brief reached the agent's input. Poll the capture (typed injection writes it
    // as the bytes arrive).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut captured = String::new();
    while Instant::now() < deadline {
        captured = std::fs::read_to_string(&capture).unwrap_or_default();
        if captured.contains("omega-last") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    session.kill();

    // The regression this guards: the card title (line 1) AND every below-the-fold detail
    // arrived. A `cmd.exe` launch-argument brief would have delivered only "CARD TITLE
    // alpha-one" and dropped everything after the first newline. Typed injection delivers the
    // whole brief - so the acceptance token and the final line are both present.
    assert!(captured.contains("alpha-one"), "missing the card title line: {captured:?}");
    assert!(
        captured.contains("quokka"),
        "the below-the-fold acceptance token was truncated - the bug is NOT fixed: {captured:?}"
    );
    assert!(
        captured.contains("gamma-three"),
        "a middle line was dropped: {captured:?}"
    );
    assert!(
        captured.contains("omega-last"),
        "the final brief line did not arrive: {captured:?}"
    );
    // NOTE on newlines: ConPTY drops a lone LF on the input path (proven for both `none` and
    // `bracketed` paste modes), so typed injection normalizes the inter-line newlines - the
    // same behavior as the proven New-Session first-prompt path (finding #3). What the fix
    // guarantees, and this asserts, is that no CONTENT is lost past the first newline; the
    // bug was total truncation to the card title, and every line's text is now present.

    let _ = std::fs::remove_dir_all(&dir);
}
