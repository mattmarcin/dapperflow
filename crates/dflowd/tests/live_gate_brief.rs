//! LIVE gate reviewer brief delivery (`gate.md` / Adversarial review; `adapters.md` /
//! Dispatch brief delivery). Opt-in (`#[ignore]`); spends a little real credit.
//!
//!   cargo build -p dflow-cli
//!   cargo test -p dflowd --test live_gate_brief -- --ignored --nocapture
//!
//! A minimal FULL gate: a real claude-haiku author commits a change, then a REAL codex
//! reviewer reviews it on the DIFFERENT harness. codex installs as a `*.cmd` shim, so its
//! session launches through `cmd.exe /c` - the launch form whose multi-line argument
//! truncates at the first newline, and the default reviewer once "reviewer != author" forces
//! a cross-model pairing (claude author -> codex reviewer). The reviewer brief carries a
//! BELOW-THE-FIRST-NEWLINE instruction that requires COMPUTING a value: the word quokka plus
//! the sum of 1234 and 4321 = 5555. 5555 appears nowhere in the brief, so the reviewer
//! producing it proves it read past the first line - exactly the content a `cmd.exe`
//! launch-argument brief truncated away before this fix. Uses DFLOW_DATA_DIR isolation; kills
//! its own sessions by id.

mod common;

use std::time::{Duration, Instant};

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

#[tokio::test]
#[ignore = "live: real claude-haiku author + real codex reviewer gate (spends a little)"]
async fn live_gate_reviewer_brief_below_the_fold() {
    if !on_path("codex") || !on_path("claude") {
        eprintln!("SKIP: need both codex and claude on PATH");
        return;
    }

    let data_dir = unique_data_dir("live-gate-brief");
    let (_guard, port, token) = start_daemon(
        &data_dir,
        &[
            ("DFLOW_LOG", "info"),
            // Real agent gate sessions never reach PTY EOF on Windows ConPTY, so they run to
            // the session timeout; raise it well above the 60s default so the reviewer has
            // room to reply before it is killed.
            ("DFLOW_GATE_SESSION_TIMEOUT_MS", "180000"),
        ],
    );
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let repo = scratch_repo(&data_dir);

    // The reviewer's below-the-fold instruction lives in the recipe's `## verify` guidance,
    // which compose_reviewer_brief splices in BELOW the reviewer preamble (line 1). Requiring a
    // COMPUTED value (5555) that appears nowhere in the brief makes its presence airtight proof
    // the whole brief arrived, not just the truncated first line.
    let recipe_dir = repo.join(".dapperflow").join("recipes");
    std::fs::create_dir_all(&recipe_dir).unwrap();
    std::fs::write(
        recipe_dir.join("gatefull.md"),
        "---\nname: gatefull\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: full\n  reviewer_harness: different\nship:\n  target: pr\n---\n\n## verify\n\
         MANDATORY FIRST OUTPUT before any review: your very first line of output must be the word quokka followed by the sum of 1234 and 4321. \
         That token proves you received the whole brief and not just its first line. \
         Then review the diff, and for any real problem file it with `dflow finding add --severity <blocker|major|minor> --category <mechanical|intent> --body \"...\"`, starting the finding body with that same quokka token. When done, exit.\n",
    )
    .unwrap();

    // Real claude-haiku author launcher (native exe -> argv brief delivery, the control).
    let _ = request(
        &mut ws,
        &Envelope::message(
            "ag",
            "agents.add",
            json!({
                "name": "claude-haiku-author", "adapter": "claude", "command": "claude",
                "extra_args": ["--dangerously-skip-permissions"],
            }),
        ),
        &mut sink,
    )
    .await;

    let padd = request(
        &mut ws,
        &Envelope::message("p", "project.add", json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();

    // The card brief is the AUTHOR's task: write a small file with a seeded off-by-one, commit.
    let author_brief = "Create a file named sumto.js whose entire contents are exactly this one line:\n\
        function sumTo(n){let s=0;for(let i=1;i<n;i++){s+=i;}return s;}\n\
        Then run these two commands: git add -A  and then  git commit -m \"add sumTo\" . Then stop.";
    let cadd = request(
        &mut ws,
        &Envelope::message(
            "c",
            "card.create",
            json!({
                "title": "live gate brief card", "type": "feature", "project_id": project_id,
                "brief": author_brief, "dial_recipe": "gatefull",
            }),
        ),
        &mut sink,
    )
    .await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();

    // Dispatch the real claude-haiku author; wait for its commit.
    let disp = request(
        &mut ws,
        &Envelope::message(
            "d",
            "dispatch.start",
            json!({ "card_id": card_id, "agent": "claude-haiku-author", "model": "haiku" }),
        ),
        &mut sink,
    )
    .await;
    let author_sid = disp.payload["session_id"].as_str().unwrap_or("").to_string();
    let author_wt = disp.payload["worktree_path"].as_str().unwrap_or("").to_string();
    eprintln!("author session {author_sid}, worktree {author_wt}");
    let committed = wait_for_commit(&author_wt, "sumto.js", Duration::from_secs(180));
    eprintln!("author committed sumto.js: {committed}");
    assert!(committed, "the claude-haiku author never committed sumto.js");
    let _ = request(
        &mut ws,
        &Envelope::message("ka", "session.kill", json!({ "session_id": author_sid })),
        &mut sink,
    )
    .await;

    // Run the FULL gate with a REAL codex reviewer on the different (shim) harness.
    let gr = request(
        &mut ws,
        &Envelope::message("g", "gate.run", json!({ "card_id": card_id, "reviewer_harness": "codex" })),
        &mut sink,
    )
    .await;
    assert_eq!(gr.msg_type, "gate.run", "gate.run failed: {gr:?}");
    eprintln!("gate.run: {}", gr.payload);

    // Find the live codex reviewer session for this card, attach, and poll its screen for 5555.
    let reviewer_sid = wait_for_reviewer_session(&mut ws, &card_id, "codex", Duration::from_secs(90))
        .await
        .expect("the gate never launched a live codex reviewer session");
    eprintln!("codex reviewer session {reviewer_sid}");
    let _ = request(
        &mut ws,
        &Envelope::message("at", "session.attach", json!({ "session_id": reviewer_sid, "cols": 120, "rows": 40 })),
        &mut sink,
    )
    .await;

    let mut seen = false;
    let mut last = String::new();
    let deadline = Instant::now() + Duration::from_secs(150);
    while Instant::now() < deadline {
        let stream = collect_output_until(&mut ws, "5555", Duration::from_secs(5)).await;
        let peek = request(
            &mut ws,
            &Envelope::message("pk", "session.peek", json!({ "session_id": reviewer_sid, "lines": 50 })),
            &mut sink,
        )
        .await;
        last = peek.payload["text"].as_str().unwrap_or("").to_string();
        if stream.contains("5555") || last.contains("5555") {
            seen = true;
            break;
        }
    }
    eprintln!("---- codex reviewer settled screen ----\n{last}\n---- end ----");

    // Bonus: a filed finding whose body carries the token is even stronger proof.
    let status = request(
        &mut ws,
        &Envelope::message("gs", "gate.status", json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    let findings = status.payload["findings"].as_array().cloned().unwrap_or_default();
    let finding_with_token = findings.iter().any(|f| f["body"].as_str().unwrap_or("").contains("5555"));
    eprintln!("filed findings: {}", findings.len());
    for f in &findings {
        eprintln!(
            "  [{}/{}] {}",
            f["severity"].as_str().unwrap_or(""),
            f["category"].as_str().unwrap_or(""),
            f["body"].as_str().unwrap_or("")
        );
    }
    eprintln!("BELOW-THE-FOLD TOKEN (5555) in reviewer output: {seen}");
    eprintln!("BELOW-THE-FOLD TOKEN (5555) in a filed finding: {finding_with_token}");

    let _ = request(
        &mut ws,
        &Envelope::message("kr", "session.kill", json!({ "session_id": reviewer_sid })),
        &mut sink,
    )
    .await;

    assert!(
        seen || finding_with_token,
        "the real codex reviewer never produced the below-the-fold computed value (5555); its \
         brief did not arrive in full past the first line"
    );
}

/// Wait until `file` exists in the worktree `dir` AND HEAD has advanced past the base commit
/// (the author actually committed, not merely wrote the file).
fn wait_for_commit(dir: &str, file: &str, timeout: Duration) -> bool {
    let path = std::path::Path::new(dir).join(file);
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["rev-list", "--count", "HEAD"])
                .output();
            if let Ok(o) = out {
                let n: i64 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0);
                if n >= 2 {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

/// Poll `session.list` for a live session on `harness` for `card_id` (the gate reviewer).
async fn wait_for_reviewer_session(
    ws: &mut Ws,
    card_id: &str,
    harness: &str,
    timeout: Duration,
) -> Option<String> {
    let mut sink = Vec::new();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let listing = request(ws, &Envelope::message("sl", "session.list", json!({})), &mut sink).await;
        if let Some(arr) = listing.payload["sessions"].as_array() {
            if let Some(s) = arr
                .iter()
                .find(|s| s["harness"] == harness && s["card_id"] == card_id && s["alive"] == true)
            {
                if let Some(id) = s["session_id"].as_str() {
                    return Some(id.to_string());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
}
