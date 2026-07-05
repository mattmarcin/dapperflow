//! Env vault + M4 wiring, end to end over the wire (`environments.md`,
//! `protocol.md` / env.*, `phase6-mcp.md` merge-time requests).
//!
//! A real `dflowd.exe` on an isolated `DFLOW_DATA_DIR`; a stub launcher
//! (`cmd.exe`) that echoes its injected env so we can prove materialization reached
//! the spawn environment, and `session.peek` to prove the scrub.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use dflow_proto::{
    CardCreated, ClientKind, ConcertmasterMinted, DispatchCancelled, DispatchStarted,
    EnvImportResult, EnvListResult, EnvSetResult, Envelope, ProjectAdded, SessionPeeked,
};

/// The dispatched stub echoes the two injected env vars and stays at the prompt, so a
/// peek captures the materialized values on screen.
const STUB_LAUNCH: &str =
    r#"["cmd.exe","/d","/k","echo PUB=%PUBLIC_URL% & echo SEC=%DB_PASSWORD% & echo done-marker"]"#;

const SECRET_VALUE: &str = "hunter2-supersecret-db-pw";
const VAR_VALUE: &str = "https://materialized.example";

#[tokio::test]
async fn vault_materializes_into_dispatch_scrubs_peek_and_shreds_on_return() {
    let data_dir = unique_data_dir("vault");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[("DFLOW_LAUNCH_STUB", STUB_LAUNCH)]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // ---- project + vault entries. ----
    let added: ProjectAdded = request(
        &mut ws,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    let project_id = added.project_id;

    // A var, a secret, and a file entry (env.set is write-only; the response is metadata).
    let set_var: EnvSetResult = request(
        &mut ws,
        &Envelope::message(
            "e1",
            "env.set",
            serde_json::json!({ "project_id": project_id, "key": "PUBLIC_URL", "kind": "var", "value": VAR_VALUE }),
        ),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    assert_eq!(set_var.entry.kind, "var");

    request(
        &mut ws,
        &Envelope::message(
            "e2",
            "env.set",
            serde_json::json!({ "project_id": project_id, "key": "DB_PASSWORD", "kind": "secret", "value": SECRET_VALUE }),
        ),
        &mut sink,
    )
    .await;
    request(
        &mut ws,
        &Envelope::message(
            "e3",
            "env.set",
            serde_json::json!({
                "project_id": project_id, "key": "dev-vars", "kind": "file",
                "value": "SECRET_TOKEN=file-materialized-secret\n", "target": ".dev.vars"
            }),
        ),
        &mut sink,
    )
    .await;

    // ---- env.list returns names + kinds, and NEVER any value. ----
    let list_env = request(
        &mut ws,
        &Envelope::message("e4", "env.list", serde_json::json!({ "project_id": project_id })),
        &mut sink,
    )
    .await;
    let raw_list = serde_json::to_string(&list_env).unwrap();
    assert!(!raw_list.contains(SECRET_VALUE), "env.list leaked a secret value: {raw_list}");
    assert!(!raw_list.contains(VAR_VALUE), "env.list leaked a var value: {raw_list}");
    let listed: EnvListResult = list_env.decode_payload().unwrap();
    assert_eq!(listed.entries.len(), 3);
    let file_entry = listed.entries.iter().find(|e| e.kind == "file").unwrap();
    assert_eq!(file_entry.target.as_deref(), Some(".dev.vars"));

    // ---- env.import classifies secret-looking keys and reports what it did. ----
    let env_file = data_dir.join("import.env");
    std::fs::write(
        &env_file,
        "PORT=3000\nSTRIPE_API_KEY=sk_test_xyz\nPUBLIC_HOST=example.com\n",
    )
    .unwrap();
    let import_env = request(
        &mut ws,
        &Envelope::message(
            "e5",
            "env.import",
            serde_json::json!({ "project_id": project_id, "path": env_file.to_string_lossy() }),
        ),
        &mut sink,
    )
    .await;
    assert!(
        !serde_json::to_string(&import_env).unwrap().contains("sk_test_xyz"),
        "env.import must not echo a value"
    );
    let report: EnvImportResult = import_env.decode_payload().unwrap();
    assert_eq!(report.imported, 3);
    assert_eq!(report.secrets, 1, "STRIPE_API_KEY is classified secret");
    assert_eq!(report.vars, 2);
    let kind_of = |k: &str| report.entries.iter().find(|e| e.key == k).map(|e| e.kind.clone());
    assert_eq!(kind_of("STRIPE_API_KEY").as_deref(), Some("secret"));
    assert_eq!(kind_of("PORT").as_deref(), Some("var"));

    // ---- dispatch materializes the vault: file on disk + vars/secrets in spawn env. ----
    let created: CardCreated = request(
        &mut ws,
        &Envelope::message(
            "c",
            "card.create",
            serde_json::json!({ "title": "Vault dispatch", "type": "feature", "project_id": project_id }),
        ),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    let card_id = created.card_id;

    let started: DispatchStarted = request(
        &mut ws,
        &Envelope::message("d", "dispatch.start", serde_json::json!({ "card_id": card_id, "harness": "stub" })),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    let worktree_path = started.worktree_path.clone();
    let dev_vars = Path::new(&worktree_path).join(".dev.vars");
    assert!(dev_vars.exists(), "the file entry must be materialized into the worktree");
    assert_eq!(
        std::fs::read_to_string(&dev_vars).unwrap(),
        "SECRET_TOKEN=file-materialized-secret\n"
    );

    // The env_materialized event carries counts and file names, never a value.
    let card_get = request(
        &mut ws,
        &Envelope::message("cg", "card.get", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await;
    let got: dflow_proto::CardGetResult = card_get.decode_payload().unwrap();
    let mat_event = got.events.iter().find(|e| e.kind == "env_materialized").expect("env_materialized event");
    // The vault holds DB_PASSWORD + STRIPE_API_KEY (2 secrets) after the import above.
    assert_eq!(mat_event.payload["secrets"].as_u64(), Some(2));
    assert!(mat_event.payload["vars"].as_u64().unwrap() >= 1);
    assert!(mat_event.payload.to_string().contains(".dev.vars"));
    assert!(!mat_event.payload.to_string().contains(SECRET_VALUE), "the event must not carry a value");

    // ---- session.peek shows the injected var, redacts the injected secret, no resize. ----
    let peek = poll_peek_until(&mut ws, &started.session_id, "PUB=", Duration::from_secs(15)).await;
    assert!(peek.text.contains(&format!("PUB={VAR_VALUE}")), "the var must reach the spawn env: {}", peek.text);
    assert!(!peek.text.contains(SECRET_VALUE), "peek must redact the secret value: {}", peek.text);
    assert!(peek.text.contains("[dflow:redacted]"), "expected the redaction marker: {}", peek.text);

    // ---- dispatch.cancel shreds the materialized secret file before the worktree returns. ----
    let cancelled: DispatchCancelled = request(
        &mut ws,
        &Envelope::message("x", "dispatch.cancel", serde_json::json!({ "card_id": card_id })),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    assert_eq!(cancelled.cancelled, 1);
    assert!(!dev_vars.exists(), "the materialized secret file must be shredded on return");

    let _ = request(&mut ws, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}

#[tokio::test]
async fn knowledge_verbs_route_on_owner_scope() {
    let data_dir = unique_data_dir("know-root");
    let repo = scratch_repo(&data_dir);
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut ws = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    let added: ProjectAdded = request(
        &mut ws,
        &Envelope::message("p", "project.add", serde_json::json!({ "path": repo.to_string_lossy() })),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    let pid = added.project_id;

    // Previously know.* was routed only for per-task agent tokens; on owner scope it now
    // resolves via the explicit project_id (`phase6-mcp.md` merge-time request 2). The
    // proof is that each verb is ROUTED (its own msg_type), not an `unsupported` error.
    let index = request(
        &mut ws,
        &Envelope::message("k1", "know.index", serde_json::json!({ "project_id": pid })),
        &mut sink,
    )
    .await;
    assert_eq!(index.msg_type, "know.index", "know.index must route on owner scope: {index:?}");

    let find = request(
        &mut ws,
        &Envelope::message("k2", "know.find", serde_json::json!({ "project_id": pid, "query": "anything" })),
        &mut sink,
    )
    .await;
    assert_eq!(find.msg_type, "know.find", "know.find must route on owner scope: {find:?}");

    let get = request(
        &mut ws,
        &Envelope::message("k3", "know.get", serde_json::json!({ "project_id": pid, "id": "does/not/exist" })),
        &mut sink,
    )
    .await;
    assert_eq!(get.msg_type, "know.get", "know.get must route on owner scope: {get:?}");

    let _ = request(&mut ws, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}

#[tokio::test]
async fn concertmaster_token_scope_is_enforced() {
    let data_dir = unique_data_dir("cm-scope");
    let (_daemon, port, token) = start_daemon(&data_dir, &[]);
    let mut root = connect_and_auth(port, &token).await;
    let mut sink = Vec::new();

    // Owner mints a Concertmaster-scoped token and is told what it withholds.
    let minted: ConcertmasterMinted = request(
        &mut root,
        &Envelope::message("m", "auth.mint_concertmaster", serde_json::json!({})),
        &mut sink,
    )
    .await
    .decode_payload()
    .unwrap();
    assert!(minted.excludes.iter().any(|e| e.contains("vault")), "excludes must name the vault: {:?}", minted.excludes);
    assert!(minted.excludes.iter().any(|e| e.contains("kill")));

    // A connection authenticated with the Concertmaster token gets the reduced surface.
    let mut cm = connect_with(port, &minted.token, ClientKind::Desktop).await;

    // Allowed: read verbs.
    let fleet = request(&mut cm, &Envelope::message("f", "fleet.status", serde_json::json!({})), &mut sink).await;
    assert_eq!(fleet.msg_type, "fleet.status", "fleet.status must be allowed: {fleet:?}");

    // Excluded: vault, kill, dispatch.cancel, agents.*, daemon.shutdown -> Forbidden.
    for (rid, verb, payload) in [
        ("v", "env.list", serde_json::json!({ "project_id": "x" })),
        ("k", "session.kill", serde_json::json!({ "session_id": "x" })),
        ("dc", "dispatch.cancel", serde_json::json!({ "card_id": "x" })),
        ("a", "agents.list", serde_json::json!({})),
        ("sd", "daemon.shutdown", serde_json::json!({})),
    ] {
        let resp = request(&mut cm, &Envelope::message(rid, verb, payload), &mut sink).await;
        assert_eq!(resp.msg_type, "error", "{verb} must be refused for the Concertmaster scope: {resp:?}");
        assert_eq!(resp.payload["code"], "forbidden", "{verb} must be a forbidden error: {resp:?}");
    }

    // session.send_verified IS routed for the Concertmaster (it steers) - not Forbidden,
    // and not `unsupported`. A bogus session id resolves to a not_found error, which
    // proves the verb reached its handler (`phase6-mcp.md` merge-time request 1).
    let steer = request(
        &mut cm,
        &Envelope::message("sv", "session.send_verified", serde_json::json!({ "session_id": "01ABC", "text": "x", "submit": true })),
        &mut sink,
    )
    .await;
    assert_ne!(steer.payload["code"], "forbidden", "send_verified must be in the Concertmaster scope: {steer:?}");
    assert_ne!(steer.payload["code"], "unsupported", "send_verified must be routed, not unsupported: {steer:?}");

    // The same verb is routed on the owner (desktop) connection too (request 1: route
    // send_verified for desktop/owner scope), reaching the handler (not_found for a
    // bogus session), not an `unsupported` reply.
    let owner_steer = request(
        &mut root,
        &Envelope::message("osv", "session.send_verified", serde_json::json!({ "session_id": "01ABC", "text": "x", "submit": true })),
        &mut sink,
    )
    .await;
    assert_eq!(owner_steer.msg_type, "error");
    assert_eq!(owner_steer.payload["code"], "not_found", "owner send_verified must route to the handler: {owner_steer:?}");

    let _ = request(&mut root, &Envelope::message("s", "daemon.shutdown", serde_json::json!({})), &mut sink).await;
}

/// Poll `session.peek` until its text contains `needle` or the deadline passes.
async fn poll_peek_until(ws: &mut Ws, session_id: &str, needle: &str, timeout: Duration) -> SessionPeeked {
    let deadline = Instant::now() + timeout;
    let mut sink = Vec::new();
    let mut last: Option<SessionPeeked> = None;
    let mut n = 0;
    while Instant::now() < deadline {
        let resp = request(
            ws,
            &Envelope::message(format!("pk{n}"), "session.peek", serde_json::json!({ "session_id": session_id, "lines": 40 })),
            &mut sink,
        )
        .await;
        n += 1;
        let peeked: SessionPeeked = resp.decode_payload().unwrap();
        if peeked.text.contains(needle) {
            return peeked;
        }
        last = Some(peeked);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    last.unwrap_or(SessionPeeked { session_id: session_id.to_string(), lines: 0, text: String::new() })
}
