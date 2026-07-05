//! Cardless (bare `session.create`) sessions keep their Projects-tree identity across
//! an app reload and a daemon restart (`product.md` / Session-first workflow; the
//! fix-sessions spike). Reproduces the core half of the "cardless session vanishes from
//! the sidebar on restart" bug: the daemon must match cwd -> project at create and
//! return `project_id` on the fleet wire for a session that has no card, both while the
//! PTY is live and after a restart marks it `interrupted`.

mod common;

use std::time::Duration;

use common::*;
use dflow_proto::{Envelope, ProjectAdded, SessionCreate, SessionCreated, SessionListResult};

#[tokio::test]
async fn cardless_session_keeps_project_identity_across_restart() {
    let data_dir = unique_data_dir("cardless");
    let repo = scratch_repo(&data_dir);
    let repo_str = repo.to_string_lossy().to_string();

    // ---- Run 1: register a project, then start a CARDLESS session in its cwd. ----
    let project_id;
    let session_id;
    {
        let (_daemon, port, token) = start_daemon(&data_dir, &[]);
        let mut ws = connect_and_auth(port, &token).await;
        let mut sink = Vec::new();

        let resp = request(
            &mut ws,
            &Envelope::message("p1", "project.add", serde_json::json!({ "path": repo_str })),
            &mut sink,
        )
        .await;
        let added: ProjectAdded = resp.decode_payload().unwrap();
        project_id = added.project_id.clone();

        // The New Session front door: session.create with cwd = the project path and NO
        // card. `cmd` is a cheap real PTY; project linkage is derived from cwd, not the
        // shell, so this exercises the exact cwd -> project match the app relies on.
        let resp = request(
            &mut ws,
            &Envelope::message(
                "s1",
                "session.create",
                SessionCreate {
                    card_id: None,
                    worktree_id: None,
                    harness: "cmd".into(),
                    first_prompt: None,
                    agent: None,
                    command: None,
                    cols: 100,
                    rows: 30,
                    cwd: Some(repo_str.clone()),
                    env: Default::default(),
                },
            ),
            &mut sink,
        )
        .await;
        assert_eq!(resp.msg_type, "session.create", "create response: {resp:?}");
        let created: SessionCreated = resp.decode_payload().unwrap();
        session_id = created.session_id.clone();

        // While the PTY is live, the fleet row must carry the cwd -> project match even
        // though the session has no card.
        let row = fleet_row(&mut ws, &mut sink, &session_id).await;
        assert!(row.alive, "session should be live in run 1: {row:?}");
        assert_eq!(row.card_id, None, "this is a cardless session: {row:?}");
        assert_eq!(
            row.project_id.as_deref(),
            Some(project_id.as_str()),
            "cardless session must be linked to its cwd's project on the wire: {row:?}"
        );

        // Simulate an app reload: a brand-new WS client (no terminal pool, nothing
        // cached) still sees the session with its project identity.
        let mut ws2 = connect_and_auth(port, &token).await;
        let mut sink2 = Vec::new();
        let row = fleet_row(&mut ws2, &mut sink2, &session_id).await;
        assert_eq!(
            row.project_id.as_deref(),
            Some(project_id.as_str()),
            "a fresh client must still see the project link (persisted, not client state): {row:?}"
        );

        // Graceful restart, like the desktop shell's Restart daemon: mark live sessions
        // interrupted and reap PTY hosts via the job object (no orphaned OpenConsole).
        let _ = request(
            &mut ws,
            &Envelope::message("q1", "daemon.shutdown", serde_json::json!({})),
            &mut sink,
        )
        .await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    let _ = std::fs::remove_file(data_dir.join("runtime.json"));

    // ---- Run 2: same data dir. The session survives as `interrupted`, still linked. ----
    let (_daemon2, port2, token2) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port2, &token2).await;
    let mut sink = Vec::new();

    let row = fleet_row(&mut ws, &mut sink, &session_id).await;
    assert_eq!(row.state, "interrupted", "must survive the restart, resumable: {row:?}");
    assert!(!row.alive, "PTY is gone after restart: {row:?}");
    assert_eq!(row.card_id, None, "still cardless: {row:?}");
    assert_eq!(
        row.project_id.as_deref(),
        Some(project_id.as_str()),
        "the Projects-tree home must survive the restart, so the row is never invisible: {row:?}"
    );

    let _ = request(
        &mut ws,
        &Envelope::message("q2", "daemon.shutdown", serde_json::json!({})),
        &mut sink,
    )
    .await;
}

/// Fetch one session's fleet row by id via `session.list`.
async fn fleet_row(
    ws: &mut Ws,
    sink: &mut Vec<u8>,
    session_id: &str,
) -> dflow_proto::SessionSummary {
    let resp = request(
        ws,
        &Envelope::message("l", "session.list", serde_json::json!({})),
        sink,
    )
    .await;
    let listing: SessionListResult = resp.decode_payload().unwrap();
    listing
        .sessions
        .into_iter()
        .find(|s| s.session_id == session_id)
        .expect("session row present in the fleet listing")
}
