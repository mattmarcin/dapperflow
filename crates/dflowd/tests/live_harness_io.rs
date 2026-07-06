//! LIVE 4-harness matrix for audit findings #2 (launch, no os error 193) and #3
//! (first prompt reaches the agent via a launch argument). Opt-in (`#[ignore]`); run with:
//!
//!   cargo build -p dflow-cli
//!   cargo test -p dflowd --test live_harness_io -- --ignored --nocapture
//!
//! Each harness is dispatched in an isolated scratch project with a trivial "reply ok"
//! brief on a cheap model, so the brief rides in as the `{prompt}` LAUNCH ARGUMENT
//! (`adapters.md` dispatch flow). We assert the session launched (a real session id, no
//! immediate 193) and capture the live screen so the evidence lands in the spike doc.
//! Every session is kept short and killed. Uses DFLOW_DATA_DIR isolation.

mod common;

use std::time::Duration;

use common::*;
use dflow_proto::Envelope;
use serde_json::json;

fn on_path(name: &str) -> bool {
    let exts = ["", ".exe", ".cmd", ".bat"];
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .any(|dir| exts.iter().any(|e| dir.join(format!("{name}{e}")).is_file()))
        })
        .unwrap_or(false)
}

struct HarnessCase {
    name: &'static str,
    adapter: &'static str,
    command: &'static str,
    model: Option<&'static str>,
    extra_args: Vec<&'static str>,
}

/// Dispatch one harness, attach, capture the live screen, and report. Returns the
/// captured screen text (evidence). Panics only on a hard launch failure (e.g. os error
/// 193 surfaced by the daemon), which is the regression this guards.
async fn run_harness(ws: &mut Ws, project_id: &str, case: &HarnessCase) -> String {
    let mut sink = Vec::new();

    // A configured launcher so pi/cursor (not built-in dispatchable) are reachable and so
    // every harness goes through the same launcher path.
    let _ = request(
        ws,
        &Envelope::message(
            "ag",
            "agents.add",
            json!({
                "name": format!("{}-live", case.name),
                "adapter": case.adapter,
                "command": case.command,
                "extra_args": case.extra_args,
            }),
        ),
        &mut sink,
    )
    .await;

    let cadd = request(
        ws,
        &Envelope::message(
            "c",
            "card.create",
            json!({
                "title": format!("{} launch proof", case.name),
                "type": "chore",
                "project_id": project_id,
                "brief": "Reply with exactly the word: ok . Then stop. Do not use any tools, do not edit files, do not ask for permission.",
            }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().expect("card_id").to_string();

    let mut disp_req = json!({
        "card_id": card_id,
        "agent": format!("{}-live", case.name),
    });
    if let Some(m) = case.model {
        disp_req["model"] = json!(m);
    }
    let disp = request(ws, &Envelope::message("d", "dispatch.start", disp_req), &mut sink).await;

    // A dispatch that hit os error 193 fails HERE with an internal error, not a session id.
    let session_id = match disp.payload["session_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            let err = disp
                .payload
                .get("error")
                .or_else(|| disp.payload.get("message"))
                .cloned()
                .unwrap_or(disp.payload.clone());
            panic!("[{}] dispatch did not launch (finding #2 regression?): {err}", case.name);
        }
    };
    eprintln!("\n==== [{}] dispatched session {session_id} ====", case.name);

    // Attach and watch the first stretch of the TUI: launch draws SOMETHING (a banner, a
    // busy footer, a trust dialog, or the reply). An os error 193 launch would instead
    // die instantly with no PTY output.
    let _ = request(
        ws,
        &Envelope::message(
            "at",
            "session.attach",
            json!({ "session_id": session_id, "cols": 120, "rows": 40 }),
        ),
        &mut sink,
    )
    .await;
    let screen = collect_output_until(ws, "esc to interrupt", Duration::from_secs(45)).await;

    // A peek gives the settled, scrubbed screen tail after a short wait (lets a slow
    // model produce its reply / footer).
    tokio::time::sleep(Duration::from_secs(6)).await;
    let peek =
        request(ws, &Envelope::message("pk", "session.peek", json!({ "session_id": session_id, "lines": 40 })), &mut sink).await;
    let peek_text = peek.payload["text"].as_str().unwrap_or("").to_string();

    // Evidence of a live launch: SOMETHING was drawn to the pty.
    assert!(
        !screen.trim().is_empty() || !peek_text.trim().is_empty(),
        "[{}] session produced no PTY output at all - launch likely failed",
        case.name
    );

    eprintln!("---- [{}] live screen (first 45s stream) ----\n{}", case.name, tail(&screen, 30));
    eprintln!("---- [{}] settled peek ----\n{peek_text}\n---- end [{}] ----", case.name, case.name);

    // Kill just this session's tree (its own pid); the daemon guard reaps the rest.
    let _ = request(ws, &Envelope::message("k", "session.kill", json!({ "session_id": session_id })), &mut sink).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    format!("{}\n{}", screen, peek_text)
}

fn tail(s: &str, lines: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// FREE probe (no credits): what does a `.cmd` shim actually receive when a multi-line
/// prompt is passed as an argument through `cmd.exe /c`? Writes a stub capture.cmd,
/// spawns it via the real session path with a multi-line arg, and reads back what the
/// batch shim saw. Proves the newline-truncation hypothesis for finding #3.
#[tokio::test]
#[ignore = "probe: spawns cmd.exe only, no credits"]
async fn probe_cmd_multiline_arg() {
    let data_dir = unique_data_dir("probe-cmd");
    let (_guard, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let cmd_path = data_dir.join("capture.cmd");
    let out_path = data_dir.join("captured.txt");
    // Echo the whole argument list to a file. Delayed-expansion-free: %* is the raw args.
    std::fs::write(&cmd_path, "@echo off\r\n(echo %*)> \"%~dp0captured.txt\"\r\n").unwrap();

    let multiline = "FIRST LINE keep me\nSECOND LINE keep me too\nTHIRD LINE also";
    let _ = request(
        &mut ws,
        &Envelope::message(
            "sc",
            "session.create",
            json!({
                "harness": "cmd",
                "command": ["cmd.exe", "/c", cmd_path.to_string_lossy(), multiline],
                "cols": 120, "rows": 40,
            }),
        ),
        &mut sink,
    )
    .await;

    // Give the batch a moment to write the file.
    for _ in 0..40 {
        if out_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let captured = std::fs::read_to_string(&out_path).unwrap_or_default();
    eprintln!("---- capture.cmd received %* ----\n{captured}\n---- end ----");
    let got_all = captured.contains("SECOND LINE") && captured.contains("THIRD LINE");
    eprintln!("MULTILINE ARG SURVIVED cmd.exe /c: {got_all}");
}

/// LIVE root-cause probe for finding #3 typed injection: launch each harness
/// INTERACTIVELY (no prompt arg), inspect the idle composer, then `session.send_verified`
/// a steer and observe whether the text lands and submits. Prints the verified-submit
/// result (submitted/attempts) and before/after peeks so the failure mode is explicit.
#[tokio::test]
#[ignore = "live: launches real sessions and sends one steer each (spends a little)"]
async fn probe_typed_injection_root_cause() {
    let data_dir = unique_data_dir("probe-typed");
    let (_guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "info")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();
    let repo = scratch_repo(&data_dir);

    let cases = vec![
        ("codex", "codex", None::<&str>, vec![]),
        ("opencode", "opencode", Some("opencode-go/glm-5.2"), vec![]),
        ("pi", "pi", None, vec![]),
        ("claude", "claude", Some("haiku"), vec!["--dangerously-skip-permissions"]),
    ];

    for (name, command, model, extra) in &cases {
        if !on_path(command) {
            eprintln!("SKIP [{name}]: not on PATH");
            continue;
        }
        let _ = request(
            &mut ws,
            &Envelope::message("ag", "agents.add", json!({
                "name": format!("{name}-typed"), "adapter": name, "command": command, "extra_args": extra,
            })),
            &mut sink,
        )
        .await;
        let mut create = json!({
            "harness": name, "agent": format!("{name}-typed"),
            "cwd": repo.to_string_lossy(), "cols": 120, "rows": 40,
        });
        if let Some(m) = model {
            // model isn't a session.create field; bake it into the launcher extra_args.
            let _ = m; // model handled below via agent update
        }
        // Rebuild the launcher WITH the model flag baked into extra_args (interactive launch
        // does not splice the model axis).
        if let Some(m) = model {
            let flag = if *name == "pi" { vec!["--model", m] } else if *name == "opencode" { vec!["-m", m] } else { vec!["--model", m] };
            let mut args: Vec<&str> = extra.clone();
            args.extend(flag);
            let _ = request(&mut ws, &Envelope::message("agu", "agents.update", json!({
                "id": format!("{name}-typed"), "extra_args": args,
            })), &mut sink).await;
        }
        let created = request(&mut ws, &Envelope::message("sc", "session.create", {
            create["harness"] = json!(name);
            create
        }), &mut sink).await;
        let sid = match created.payload["session_id"].as_str() { Some(s) => s.to_string(), None => { eprintln!("[{name}] create failed: {}", created.payload); continue; } };
        eprintln!("\n######## [{name}] interactive session {sid} ########");
        let _ = request(&mut ws, &Envelope::message("at", "session.attach", json!({ "session_id": sid, "cols": 120, "rows": 40 })), &mut sink).await;
        // Let the TUI settle and answer any trust dialog.
        tokio::time::sleep(Duration::from_secs(12)).await;
        let before = request(&mut ws, &Envelope::message("pk", "session.peek", json!({ "session_id": sid, "lines": 30 })), &mut sink).await;
        eprintln!("---- [{name}] idle composer (before steer) ----\n{}\n----", before.payload["text"].as_str().unwrap_or(""));

        let steer = request(&mut ws, &Envelope::message("sv", "session.send_verified", json!({
            "session_id": sid, "text": "Reply with exactly the word: ok", "submit": true,
        })), &mut sink).await;
        eprintln!("---- [{name}] send_verified result: {} ----", steer.payload);

        tokio::time::sleep(Duration::from_secs(8)).await;
        let after = request(&mut ws, &Envelope::message("pk", "session.peek", json!({ "session_id": sid, "lines": 30 })), &mut sink).await;
        eprintln!("---- [{name}] after steer ----\n{}\n---- end [{name}] ----", after.payload["text"].as_str().unwrap_or(""));

        let _ = request(&mut ws, &Envelope::message("k", "session.kill", json!({ "session_id": sid })), &mut sink).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// LIVE New-Session first-prompt (TYPED, readiness-gated) + steer acceptance for finding
/// #3. For each harness: `session.create` with a single-line first prompt (the standing
/// dflow guidance is prepended for codex/opencode/pi, so the TYPED content is multi-line),
/// confirm the agent receives it and replies, then send a mid-session steer and confirm it
/// lands too. Captures full transcripts for the spike doc.
#[tokio::test]
#[ignore = "live: New-Session first prompt + steer on real agents (spends a little)"]
async fn live_new_session_first_prompt_and_steer() {
    let data_dir = unique_data_dir("live-newsess");
    let (_guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "info")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();
    let repo = scratch_repo(&data_dir);
    let padd = request(&mut ws, &Envelope::message("p", "project.add", json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let _project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    let cases = vec![
        ("claude", "claude", Some("haiku"), vec!["--dangerously-skip-permissions"]),
        ("codex", "codex", None::<&str>, vec![]),
        ("opencode", "opencode", Some("opencode-go/glm-5.2"), vec![]),
        ("pi", "pi", None, vec![]),
    ];

    for (name, command, model, extra) in &cases {
        if !on_path(command) {
            eprintln!("SKIP [{name}]: not on PATH");
            continue;
        }
        let mut args: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
        if let Some(m) = model {
            let flag = if *name == "pi" || *name == "claude" { "--model" } else { "-m" };
            args.push(flag.to_string());
            args.push(m.to_string());
        }
        let _ = request(&mut ws, &Envelope::message("ag", "agents.add", json!({
            "name": format!("{name}-ns"), "adapter": name, "command": command, "extra_args": args,
        })), &mut sink).await;

        let created = request(&mut ws, &Envelope::message("sc", "session.create", json!({
            "harness": name, "agent": format!("{name}-ns"), "cwd": repo.to_string_lossy(),
            "cols": 120, "rows": 40,
            "first_prompt": "Reply with exactly the word: pineapple . Then stop. Do not use tools.",
        })), &mut sink).await;
        let sid = match created.payload["session_id"].as_str() { Some(s) => s.to_string(), None => { eprintln!("[{name}] create failed: {}", created.payload); continue; } };
        let queued = created.payload["first_prompt_queued"].as_bool().unwrap_or(false);
        eprintln!("\n######## [{name}] New-Session {sid} first_prompt_queued={queued} ########");
        let _ = request(&mut ws, &Envelope::message("at", "session.attach", json!({ "session_id": sid, "cols": 120, "rows": 40 })), &mut sink).await;

        // Give the first prompt time to be typed (after readiness) and answered.
        tokio::time::sleep(Duration::from_secs(30)).await;
        let peek1 = request(&mut ws, &Envelope::message("pk", "session.peek", json!({ "session_id": sid, "lines": 40 })), &mut sink).await;
        let t1 = peek1.payload["text"].as_str().unwrap_or("");
        eprintln!("---- [{name}] after first prompt (looking for 'pineapple') ----\n{t1}\n----");
        eprintln!("[{name}] FIRST-PROMPT REPLY SEEN: {}", t1.to_lowercase().contains("pineapple"));

        // Mid-session steer.
        let steer = request(&mut ws, &Envelope::message("sv", "session.send_verified", json!({
            "session_id": sid, "text": "Now reply with exactly the word: watermelon", "submit": true,
        })), &mut sink).await;
        eprintln!("[{name}] steer result: {}", steer.payload);
        tokio::time::sleep(Duration::from_secs(25)).await;
        let peek2 = request(&mut ws, &Envelope::message("pk", "session.peek", json!({ "session_id": sid, "lines": 40 })), &mut sink).await;
        let t2 = peek2.payload["text"].as_str().unwrap_or("");
        eprintln!("---- [{name}] after steer (looking for 'watermelon') ----\n{t2}\n---- end [{name}] ----");
        eprintln!("[{name}] STEER REPLY SEEN: {}", t2.to_lowercase().contains("watermelon"));

        let _ = request(&mut ws, &Envelope::message("k", "session.kill", json!({ "session_id": sid })), &mut sink).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[tokio::test]
#[ignore = "live: launches real claude/codex/opencode/pi sessions (spends credits)"]
async fn live_four_harness_launch_and_first_prompt() {
    let data_dir = unique_data_dir("live-harness-io");
    let (_guard, port, token) = start_daemon(&data_dir, &[("DFLOW_LOG", "info")]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let repo = scratch_repo(&data_dir);
    let padd = request(
        &mut ws,
        &Envelope::message("p", "project.add", json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    let cases = vec![
        HarnessCase { name: "claude", adapter: "claude", command: "claude", model: Some("haiku"), extra_args: vec!["--dangerously-skip-permissions"] },
        HarnessCase { name: "codex", adapter: "codex", command: "codex", model: None, extra_args: vec![] },
        HarnessCase { name: "opencode", adapter: "opencode", command: "opencode", model: Some("opencode-go/glm-5.2"), extra_args: vec![] },
        HarnessCase { name: "pi", adapter: "pi", command: "pi", model: None, extra_args: vec![] },
    ];

    let mut launched = Vec::new();
    for case in &cases {
        if !on_path(case.command) {
            eprintln!("SKIP [{}]: {} not on PATH", case.name, case.command);
            continue;
        }
        let _ = run_harness(&mut ws, &project_id, case).await;
        launched.push(case.name);
    }

    eprintln!("\n==== LAUNCHED WITHOUT os error 193: {launched:?} ====");
    assert!(!launched.is_empty(), "no harness CLIs were available to test");
}
