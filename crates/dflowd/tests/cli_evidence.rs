//! Evidence capture (opt-in, `#[ignore]`): run the real `dflow` binary through every
//! verb against a live daemon and print each verb's exact stdout, for the phase3 spike.
//!
//! Run: `cargo build -p dflow-cli && cargo test -p dflowd --test cli_evidence -- --ignored --nocapture`

mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use dflow_proto::Envelope;

const STUB: &str = r#"["cmd.exe","/d","/k","echo DTOKEN=%DFLOW_TOKEN%"]"#;

#[tokio::test]
#[ignore = "evidence capture; run explicitly with --ignored"]
async fn capture_cli_output() {
    let dflow = std::path::Path::new(env!("CARGO_BIN_EXE_dflowd"))
        .parent()
        .unwrap()
        .join(if cfg!(windows) { "dflow.exe" } else { "dflow" });
    assert!(dflow.exists(), "build dflow first: cargo build -p dflow-cli");

    let data_dir = unique_data_dir("cli-evidence");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB)]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let padd = request(&mut root, &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })), &mut sink).await;
    let project_id = padd.payload["project_id"].as_str().unwrap().to_string();
    // Seed a Digest so `dflow card`/`dflow know` show one.
    std::fs::create_dir_all(repo.join("docs/knowledge")).unwrap();
    std::fs::write(repo.join("docs/knowledge/index.md"), "# Project knowledge - repo\n\n## Digest\ntailwind; accounts soft-delete 90d; check docs/theming.md\n\n## Catalog\n").unwrap();
    let cadd = request(&mut root, &Envelope::message("c", "card.create", serde_json::json!({
        "title": "Add dark mode toggle", "type": "feature", "project_id": project_id,
        "brief": "Add a theme toggle.\n\n## Acceptance\n- toggle persists across sessions\n- respects system preference by default\n- no flash of wrong theme on load\n"
    })), &mut sink).await;
    let card_id = cadd.payload["card_id"].as_str().unwrap().to_string();
    let disp = request(&mut root, &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })), &mut sink).await;
    let session_id = disp.payload["session_id"].as_str().unwrap().to_string();

    tokio::time::sleep(Duration::from_secs(2)).await;
    let attached = request(&mut root, &Envelope::message("a", "session.attach", serde_json::json!({ "session_id": session_id, "cols": 120, "rows": 32 })), &mut sink).await;
    let replay = base64::engine::general_purpose::STANDARD.decode(attached.payload["replay_base64"].as_str().unwrap()).unwrap();
    let screen = String::from_utf8_lossy(&replay);
    let start = screen.find("DTOKEN=").unwrap() + 7;
    let task_token: String = screen[start..].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    let endpoint = format!("ws://127.0.0.1:{port}/ws");

    let run = |args: &[&str], stdin: Option<&str>| -> (String, i32) {
        use std::io::Write;
        let mut cmd = std::process::Command::new(&dflow);
        cmd.args(args)
            .env("DFLOW_TOKEN", &task_token)
            .env("DFLOW_ENDPOINT", &endpoint)
            .env("DFLOW_CARD", &card_id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        if let Some(s) = stdin {
            child.stdin.take().unwrap().write_all(s.as_bytes()).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (text, out.status.code().unwrap_or(-1))
    };

    let show = |label: &str, args: &[&str], stdin: Option<&str>| {
        let (text, code) = run(args, stdin);
        println!("\n$ dflow {}", args.join(" "));
        print!("{text}");
        println!("[exit {code}]  # {label}");
    };

    println!("======== dflow CLI evidence (M2) ========");
    show("bare: current card/state/next", &[], None);
    show("card brief + acceptance + digest", &["card"], None);
    show("tier-1 self-report", &["status", "working", "wiring the reducer"], None);
    show("session-strip note", &["card", "note", "toggling theme provider"], None);
    show("file a follow-up card", &["card", "create", "--title", "extract ThemeProvider", "--type", "chore"], None);
    show("record a durable note", &["know", "add", "--type", "gotcha", "--title", "theme flash on load", "--stdin"], Some("next-themes must be imported in _app to avoid a flash of the wrong theme.\n"));
    show("knowledge index", &["know"], None);
    show("search knowledge", &["know", "find", "theme"], None);
    show("read one note", &["know", "get", "gotchas/theme-flash-on-load"], None);
    show("help", &["help", "know"], None);
    // Error-path evidence (structured stderr + exit codes).
    show("usage error (blocked needs a note) -> exit 2", &["status", "blocked"], None);
    let (nf, code) = run(&["status", "done", "shipped the toggle"], None);
    println!("\n$ dflow status done \"shipped the toggle\"");
    print!("{nf}");
    println!("[exit {code}]  # stage-advance");
}
