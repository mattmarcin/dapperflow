//! Store round-trip, migration, event-stream, and reconciliation tests.
//!
//! File-backed tests use a unique temp path so a WAL database can be closed and
//! reopened (simulating a daemon restart) without colliding with any other run.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dflow_proto::CheckCmd;

use super::*;

fn temp_db_path(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-store-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("store.db")
}

/// A project + card scaffold shared by tests that need a card to hang rows off.
fn project_and_card(store: &Store) -> (String, String) {
    let project = store.add_project("C:/repos/acme", "Acme", "main", "pr").unwrap();
    let card = store
        .create_card(NewCard { project_id: Some(project.id.clone()), title: "Fix login".into(), ..Default::default() })
        .unwrap();
    (project.id, card.id)
}

#[test]
fn opens_and_migrates_to_latest() {
    let path = temp_db_path("migrate");
    let store = Store::open(&path).unwrap();
    let uv: i64 = store.lock().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv, SCHEMA_VERSION, "fresh open should be at the latest schema");
    drop(store);
    // Reopening applies no migrations and stays at the latest version.
    let store2 = Store::open(&path).unwrap();
    let uv2: i64 = store2.lock().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
    assert_eq!(uv2, SCHEMA_VERSION);
}

#[test]
fn refuses_a_newer_schema() {
    let path = temp_db_path("newer");
    let store = Store::open(&path).unwrap();
    drop(store);
    // Pretend a future daemon wrote a newer schema.
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1).unwrap();
    drop(conn);
    let err = match Store::open(&path) {
        Ok(_) => panic!("expected the store to refuse a newer schema"),
        Err(e) => e,
    };
    assert!(
        matches!(err, StoreError::SchemaTooNew { found, supported }
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION),
        "expected SchemaTooNew, got {err:?}"
    );
}

#[test]
fn project_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let p = store.add_project("C:/repos/web", "Web", "main", "pr").unwrap();
    assert_eq!(p.name, "Web");
    assert_eq!(p.default_branch, "main");
    assert!(p.check_cmds.is_empty());

    let fetched = store.get_project(&p.id).unwrap().unwrap();
    assert_eq!(fetched.path, "C:/repos/web");
    let by_path = store.get_project_by_path("C:/repos/web").unwrap().unwrap();
    assert_eq!(by_path.id, p.id);

    let updated = store
        .update_project(
            &p.id,
            Some("local_only"),
            Some(&[CheckCmd { name: "test".into(), cmd: "cargo test".into() }]),
            Some("standard"),
        )
        .unwrap();
    assert_eq!(updated.mode, "local_only");
    assert_eq!(updated.default_recipe.as_deref(), Some("standard"));
    assert_eq!(updated.check_cmds.len(), 1);
    assert_eq!(updated.check_cmds[0].cmd, "cargo test");

    assert_eq!(store.list_projects().unwrap().len(), 1);
}

#[test]
fn duplicate_project_path_is_rejected() {
    let store = Store::open_in_memory().unwrap();
    store.add_project("C:/repos/dup", "Dup", "main", "pr").unwrap();
    let err = store.add_project("C:/repos/dup", "Dup2", "main", "pr").unwrap_err();
    assert!(matches!(err, StoreError::Sqlite(_)), "unique path violation expected, got {err:?}");
}

#[test]
fn card_lifecycle_appends_events() {
    let store = Store::open_in_memory().unwrap();
    let (project_id, card_id) = project_and_card(&store);

    // `created` event exists.
    let events = store.card_events(&card_id, None, 50).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, event_kind::CREATED);

    // Move appends `moved` with from/to.
    let moved = store.move_card(&card_id, "performing").unwrap();
    assert_eq!(moved.lane, "performing");
    let events = store.card_events(&card_id, None, 50).unwrap();
    assert_eq!(events[0].kind, event_kind::MOVED, "newest first");
    assert_eq!(events[0].payload["from"], "inbox");
    assert_eq!(events[0].payload["to"], "performing");

    // Update appends `shaped`.
    let updated = store
        .update_card(&card_id, CardPatch { brief: Some("do the thing".into()), ..Default::default() })
        .unwrap();
    assert_eq!(updated.brief.as_deref(), Some("do the thing"));
    let events = store.card_events(&card_id, None, 50).unwrap();
    assert_eq!(events[0].kind, event_kind::SHAPED);

    // Query by project and by lane.
    let by_project =
        store.query_cards(&CardQueryFilter { project_id: Some(project_id), ..Default::default() }).unwrap();
    assert_eq!(by_project.len(), 1);
    let by_lane =
        store.query_cards(&CardQueryFilter { lane: Some("performing".into()), ..Default::default() }).unwrap();
    assert_eq!(by_lane.len(), 1);
    let none =
        store.query_cards(&CardQueryFilter { lane: Some("done".into()), ..Default::default() }).unwrap();
    assert!(none.is_empty());
}

#[test]
fn event_stream_is_ordered_and_resumable() {
    let store = Store::open_in_memory().unwrap();
    let (_project_id, card_id) = project_and_card(&store);
    // created + three moves = four events.
    store.move_card(&card_id, "a").unwrap();
    store.move_card(&card_id, "b").unwrap();
    store.move_card(&card_id, "c").unwrap();

    let all = store.events_after(None, 100).unwrap();
    assert_eq!(all.len(), 4);
    // Cursors (ULIDs) are strictly increasing.
    for pair in all.windows(2) {
        assert!(pair[0].id < pair[1].id, "events must be ULID-ordered");
    }

    // Resume from the second cursor: only the last two remain.
    let cursor = all[1].id.clone();
    let rest = store.events_after(Some(&cursor), 100).unwrap();
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0].id, all[2].id);

    assert_eq!(store.latest_event_cursor().unwrap().as_deref(), Some(all[3].id.as_str()));
}

#[test]
fn reconcile_marks_live_sessions_interrupted_across_reopen() {
    let path = temp_db_path("reconcile");
    let card_id;
    {
        // First "daemon run": create a working session, then the process dies.
        let store = Store::open(&path).unwrap();
        let (_p, c) = project_and_card(&store);
        card_id = c.clone();
        store
            .create_session(NewSession {
                id: ulid::Ulid::new().to_string(),
                card_id: Some(c.clone()),
                project_id: None,
                cwd: None,
                harness: "claude".into(),
                model: None,
                effort: None,
                state: session_state::WORKING.into(),
                worktree_id: None,
                scrollback_path: "ring".into(),
                first_prompt: Some("do it".into()),
                resumed_from: None,
                title: None,
                agent_id: None,
            })
            .unwrap();
        // A separately finished session must not be touched by reconciliation.
        store
            .create_session(NewSession {
                id: ulid::Ulid::new().to_string(),
                card_id: Some(c.clone()),
                project_id: None,
                cwd: None,
                harness: "claude".into(),
                model: None,
                effort: None,
                state: session_state::DONE.into(),
                worktree_id: None,
                scrollback_path: "ring2".into(),
                first_prompt: None,
                resumed_from: None,
                title: None,
                agent_id: None,
            })
            .unwrap();
    }

    // Second "daemon run": reopen the same DB and reconcile.
    let store = Store::open(&path).unwrap();
    let interrupted = store.reconcile_interrupted().unwrap();
    assert_eq!(interrupted.len(), 1, "only the working session is interrupted");

    let rows = store.card_session_rows(&card_id).unwrap();
    let states: Vec<&str> = rows.iter().map(|r| r.state.as_str()).collect();
    assert!(states.contains(&session_state::INTERRUPTED));
    assert!(states.contains(&session_state::DONE));

    // A state_changed event with reason daemon_restart was appended.
    let events = store.card_events(&card_id, None, 50).unwrap();
    let has_reconcile = events.iter().any(|e| {
        e.kind == event_kind::STATE_CHANGED && e.payload["reason"] == "daemon_restart"
    });
    assert!(has_reconcile, "reconcile should append a state_changed event");

    // Reconciling again is a no-op (interrupted is terminal).
    assert!(store.reconcile_interrupted().unwrap().is_empty());
}

#[test]
fn cardless_session_persists_with_project_linkage() {
    // Phase 2: bare session.create sessions persist (card_id nullable migration) with a
    // cwd->project match, and finalize/reconcile work without a card timeline.
    let path = temp_db_path("cardless");
    let sid = ulid::Ulid::new().to_string();
    let project_id;
    {
        let store = Store::open(&path).unwrap();
        let project = store.add_project("C:/repos/acme", "Acme", "main", "pr").unwrap();
        project_id = project.id.clone();
        let row = store
            .create_session(NewSession {
                id: sid.clone(),
                card_id: None,
                project_id: Some(project.id.clone()),
                cwd: Some("C:/repos/acme/sub".into()),
                harness: "claude".into(),
                state: session_state::WORKING.into(),
                scrollback_path: "ring".into(),
                first_prompt: Some("bare prompt".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(row.card_id.is_none(), "cardless session has no card");
        assert_eq!(row.project_id.as_deref(), Some(project_id.as_str()));
        assert_eq!(row.cwd.as_deref(), Some("C:/repos/acme/sub"));
        // A cardless state transition updates the row but appends no card event.
        store.mark_session_needs_input(&sid, "trust_dialog", 0).unwrap();
        assert_eq!(store.get_session(&sid).unwrap().unwrap().state, session_state::NEEDS_INPUT);
        assert!(store.list_needs_you(true).unwrap().is_empty(), "cardless raises no Needs You");
    }
    // Reopen (daemon restart): the still-working cardless row reconciles to interrupted.
    let store = Store::open(&path).unwrap();
    let interrupted = store.reconcile_interrupted().unwrap();
    assert!(interrupted.contains(&sid), "cardless session is reconciled too");
    assert_eq!(store.get_session(&sid).unwrap().unwrap().state, session_state::INTERRUPTED);
    assert_eq!(store.get_session(&sid).unwrap().unwrap().project_id.as_deref(), Some(project_id.as_str()));
}

#[test]
fn cursor_detection_corrects_editor_shim() {
    // Phase 2 correction: a stale detected `cursor` launcher pointing at the desktop
    // editor shim is repointed at cursor-agent when found, and disabled when absent.
    let store = Store::open_in_memory().unwrap();
    // Simulate the Phase 1.5 bug: a detected cursor launcher aimed at the editor shim.
    store
        .insert_agent(NewAgent {
            name: "cursor".into(),
            adapter: "cursor".into(),
            command: r"C:\Users\m\AppData\Local\Programs\cursor\resources\app\bin\cursor.CMD".into(),
            extra_args: Vec::new(),
            extra_env: BTreeMap::new(),
            source: agent_source::DETECTED.into(),
            detected_version: Some("3.9.16".into()),
            enabled: true,
        })
        .unwrap();

    // A detect run that finds cursor-agent repoints the command and keeps it enabled.
    store
        .apply_detection(vec![DetectedCli {
            name: "cursor".into(),
            adapter: "cursor".into(),
            command: r"C:\Users\m\AppData\Local\cursor-agent\cursor-agent.cmd".into(),
            version: Some("2026.07.01-41b2de7".into()),
        }])
        .unwrap();
    let fixed = store.get_agent_by_name("cursor").unwrap().unwrap();
    assert!(fixed.command.to_lowercase().contains("cursor-agent"), "command repointed to cursor-agent");
    assert!(fixed.enabled);

    // Reset to the editor shim, then a detect run that does NOT find cursor-agent must
    // disable it so it never launches the GUI.
    store
        .update_agent(
            &fixed.id,
            AgentPatch {
                command: Some(r"C:\Programs\cursor\bin\cursor.cmd".into()),
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    store.apply_detection(vec![]).unwrap();
    let disabled = store.get_agent_by_name("cursor").unwrap().unwrap();
    assert!(!disabled.enabled, "editor-shim cursor launcher is disabled when cursor-agent is absent");
}

#[test]
fn needs_you_raise_and_resolve() {
    let store = Store::open_in_memory().unwrap();
    let (_p, card_id) = project_and_card(&store);

    let item = store.raise_needs_you(&card_id, "agent_blocked", "needs_input:s1", 42).unwrap();
    assert_eq!(item.score, 42);
    assert!(item.resolved_at.is_none());
    assert_eq!(store.list_needs_you(true).unwrap().len(), 1);

    // Re-raising the same dedupe key updates in place, never duplicates.
    store.raise_needs_you(&card_id, "agent_blocked", "needs_input:s1", 99).unwrap();
    let open = store.list_needs_you(true).unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].score, 99);

    let resolved = store.resolve_needs_you(&card_id, "needs_input:s1", "ui").unwrap();
    assert!(resolved.is_some());
    assert!(store.list_needs_you(true).unwrap().is_empty());

    // Raised + resolved events landed on the card timeline.
    let events = store.card_events(&card_id, None, 50).unwrap();
    assert!(events.iter().any(|e| e.kind == event_kind::NEEDS_YOU_RAISED));
    assert!(events.iter().any(|e| e.kind == event_kind::NEEDS_YOU_RESOLVED));
}

#[test]
fn needs_input_transition_is_atomic() {
    let store = Store::open_in_memory().unwrap();
    let (_p, card_id) = project_and_card(&store);
    let sid = ulid::Ulid::new().to_string();
    store
        .create_session(NewSession {
            id: sid.clone(),
            card_id: Some(card_id.clone()),
            project_id: None,
            cwd: None,
            harness: "claude".into(),
            model: None,
            effort: None,
            state: session_state::WORKING.into(),
            worktree_id: None,
            scrollback_path: "ring".into(),
            first_prompt: None,
            resumed_from: None,
            title: None,
            agent_id: None,
        })
        .unwrap();

    store.mark_session_needs_input(&sid, "agent_blocked", 10).unwrap();
    assert_eq!(store.get_session(&sid).unwrap().unwrap().state, session_state::NEEDS_INPUT);
    assert_eq!(store.list_needs_you(true).unwrap().len(), 1);

    store.clear_session_needs_input(&sid, session_state::WORKING).unwrap();
    assert_eq!(store.get_session(&sid).unwrap().unwrap().state, session_state::WORKING);
    assert!(store.list_needs_you(true).unwrap().is_empty());
}

#[test]
fn session_rename_persists_title() {
    let store = Store::open_in_memory().unwrap();
    let (_p, card_id) = project_and_card(&store);
    let sid = ulid::Ulid::new().to_string();
    store
        .create_session(NewSession {
            id: sid.clone(),
            card_id: Some(card_id),
            project_id: None,
            cwd: None,
            harness: "claude".into(),
            model: None,
            effort: None,
            state: session_state::WORKING.into(),
            worktree_id: None,
            scrollback_path: "ring".into(),
            first_prompt: None,
            resumed_from: None,
            title: None,
            agent_id: None,
        })
        .unwrap();
    assert!(store.set_session_title(&sid, "Login bug").unwrap());
    assert_eq!(store.get_session(&sid).unwrap().unwrap().title.as_deref(), Some("Login bug"));
    // Empty clears back to null.
    assert!(store.set_session_title(&sid, "").unwrap());
    assert!(store.get_session(&sid).unwrap().unwrap().title.is_none());
}

// ---- agents (configured launchers, Phase 1.5) ----

use std::collections::BTreeMap;

use crate::agents::DetectedCli;
use crate::store::agent_source;

fn custom_agent(name: &str, adapter: &str, command: &str) -> NewAgent {
    NewAgent {
        name: name.into(),
        adapter: adapter.into(),
        command: command.into(),
        extra_args: Vec::new(),
        extra_env: BTreeMap::new(),
        source: agent_source::CUSTOM.into(),
        detected_version: None,
        enabled: true,
    }
}

#[test]
fn agent_crud_round_trip() {
    let store = Store::open_in_memory().unwrap();

    // cc-alt: the canonical custom launcher (claude family, second config dir).
    let mut env = BTreeMap::new();
    env.insert("CLAUDE_CONFIG_DIR".to_string(), "/home/me/.claude-alt".to_string());
    let agent = store
        .insert_agent(NewAgent {
            extra_env: env.clone(),
            ..custom_agent("cc-alt", "claude", "claude")
        })
        .unwrap();
    assert_eq!(agent.source, agent_source::CUSTOM);
    assert!(agent.enabled);
    assert!(!agent.caution, "no dangerous args yet");
    assert_eq!(agent.extra_env.get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/home/me/.claude-alt"));

    // Reference resolution: by id and by name both land the same row.
    assert_eq!(store.resolve_agent(&agent.id).unwrap().unwrap().id, agent.id);
    assert_eq!(store.resolve_agent("cc-alt").unwrap().unwrap().id, agent.id);
    assert!(store.resolve_agent("nope").unwrap().is_none());

    // Update: add a dangerous default arg -> caution flips on in the wire entity.
    let updated = store
        .update_agent(
            &agent.id,
            AgentPatch { extra_args: Some(vec!["--dangerously-skip-permissions".into()]), ..Default::default() },
        )
        .unwrap();
    assert!(updated.caution, "dangerous extra arg must compute caution");
    assert_eq!(updated.extra_args, vec!["--dangerously-skip-permissions"]);

    // Enabled toggle.
    let disabled = store.set_agent_enabled(&agent.id, false).unwrap();
    assert!(!disabled.enabled);
    assert!(!store.get_agent(&agent.id).unwrap().unwrap().enabled, "toggle persists");

    // Unique name is enforced.
    let dup = store.insert_agent(custom_agent("cc-alt", "claude", "claude")).unwrap_err();
    assert!(matches!(dup, StoreError::Sqlite(_)), "duplicate name should violate UNIQUE, got {dup:?}");

    assert_eq!(store.list_agents().unwrap().len(), 1);
}

#[test]
fn remove_agent_refuses_while_a_session_is_live() {
    let store = Store::open_in_memory().unwrap();
    let (_p, card_id) = project_and_card(&store);
    let agent = store.insert_agent(custom_agent("cc-alt", "claude", "claude")).unwrap();

    // A live session references the launcher.
    let sid = ulid::Ulid::new().to_string();
    store
        .create_session(NewSession {
            id: sid.clone(),
            card_id: Some(card_id.clone()),
            project_id: None,
            cwd: None,
            harness: "claude".into(),
            model: None,
            effort: None,
            state: session_state::WORKING.into(),
            worktree_id: None,
            scrollback_path: "ring".into(),
            first_prompt: None,
            resumed_from: None,
            title: None,
            agent_id: Some(agent.id.clone()),
        })
        .unwrap();

    let err = store.remove_agent(&agent.id).unwrap_err();
    assert!(
        matches!(err, StoreError::Invalid(ref m) if m.contains("disable it instead")),
        "removal must be refused with a disable suggestion, got {err:?}"
    );
    assert!(store.get_agent(&agent.id).unwrap().is_some(), "refused removal keeps the row");

    // Once the session ends, removal succeeds.
    store.finalize_session(&sid, session_state::DONE).unwrap();
    let removed = store.remove_agent(&agent.id).unwrap();
    assert_eq!(removed.name, "cc-alt");
    assert!(store.get_agent(&agent.id).unwrap().is_none());
}

#[test]
fn detection_creates_updates_and_preserves() {
    let store = Store::open_in_memory().unwrap();

    // Fresh detection: two CLIs found -> two new enabled detected launchers.
    let outcome = store
        .apply_detection(vec![
            DetectedCli {
                name: "claude".into(),
                adapter: "claude".into(),
                command: "C:/tools/claude.cmd".into(),
                version: Some("2.1.200".into()),
            },
            DetectedCli {
                name: "codex".into(),
                adapter: "codex".into(),
                command: "C:/tools/codex.exe".into(),
                version: Some("codex-cli 0.142.5".into()),
            },
        ])
        .unwrap();
    assert!(outcome.found.iter().all(|(_, created)| *created), "both are newly created");
    let claude = store.get_agent_by_name("claude").unwrap().unwrap();
    assert_eq!(claude.source, agent_source::DETECTED);
    assert!(claude.enabled);
    assert_eq!(claude.detected_version.as_deref(), Some("2.1.200"));

    // The user customizes the detected claude launcher (extra env + a rename-safe edit).
    let mut env = BTreeMap::new();
    env.insert("CLAUDE_CODE_ENABLE_X".to_string(), "1".to_string());
    store
        .update_agent(
            &claude.id,
            AgentPatch {
                extra_args: Some(vec!["--model".into(), "opus".into()]),
                extra_env: Some(env.clone()),
                ..Default::default()
            },
        )
        .unwrap();

    // A user adds a custom launcher that happens to reuse the codex family.
    store
        .insert_agent(NewAgent {
            extra_args: vec!["--dangerously-skip-permissions".into()],
            ..custom_agent("codex-yolo", "codex", "codex")
        })
        .unwrap();

    // Re-detect: claude's version bumps and command refreshes, but the user's
    // extra_args/extra_env survive; the custom launcher is untouched; nothing new.
    let again = store
        .apply_detection(vec![DetectedCli {
            name: "claude".into(),
            adapter: "claude".into(),
            command: "C:/tools2/claude.cmd".into(),
            version: Some("2.2.0".into()),
        }])
        .unwrap();
    assert!(again.found.iter().all(|(_, created)| !*created), "re-detect creates nothing");

    let claude2 = store.get_agent_by_name("claude").unwrap().unwrap();
    assert_eq!(claude2.detected_version.as_deref(), Some("2.2.0"), "version refreshed");
    assert_eq!(claude2.command, "C:/tools2/claude.cmd", "command refreshed");
    assert_eq!(claude2.extra_args, vec!["--model", "opus"], "user extra_args preserved");
    assert_eq!(claude2.extra_env.get("CLAUDE_CODE_ENABLE_X").map(String::as_str), Some("1"));

    let custom = store.get_agent_by_name("codex-yolo").unwrap().unwrap();
    assert_eq!(custom.source, agent_source::CUSTOM);
    assert!(custom.caution, "custom launcher's dangerous arg still flagged");
    assert_eq!(custom.command, "codex", "custom command untouched by detection");
}

/// Every `card_events` payload is scrubbed of live materialized-secret values before it
/// is persisted, as defense in depth (`security.md` / Event payloads and timelines).
#[test]
fn event_payload_scrubs_registered_secret() {
    let store = Store::open_in_memory().unwrap();
    let (_pid, card) = project_and_card(&store);

    // A distinctive value so this test never collides with any other running in
    // parallel against the process-wide registry.
    let secret = "DFLOW-EVENT-SCRUB-9f3ac71b-secret-value";
    crate::secret::registry().register("test-event-scrub", vec![secret.to_string()]);

    let event = store
        .append_card_event(
            &card,
            event_kind::BLOCKED,
            serde_json::json!({ "note": format!("agent leaked {secret} into a payload") }),
        )
        .unwrap();

    // The returned event, the durable row, and the broadcast copy are all redacted.
    let note = event.payload["note"].as_str().unwrap();
    assert!(!note.contains(secret), "event payload must be scrubbed: {note}");
    assert!(note.contains("[dflow:redacted]"), "expected redaction marker: {note}");

    let stored = store.events_after(None, 100).unwrap();
    let blocked = stored.iter().find(|e| e.kind == event_kind::BLOCKED).unwrap();
    assert!(!blocked.payload.to_string().contains(secret), "durable row must not hold the value");

    crate::secret::registry().unregister("test-event-scrub");
}

// ---- migration 0007: settings, phone tokens, round schedules ----

#[test]
fn settings_upsert_get_delete_and_bool() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.get_setting(setting_key::LAN_PORT).unwrap(), None);
    assert!(!store.get_bool_setting(setting_key::LAN_ENABLED).unwrap());

    store.set_setting(setting_key::LAN_ENABLED, "1").unwrap();
    store.set_setting(setting_key::LAN_PORT, "8790").unwrap();
    assert!(store.get_bool_setting(setting_key::LAN_ENABLED).unwrap());
    assert_eq!(store.get_setting(setting_key::LAN_PORT).unwrap().as_deref(), Some("8790"));

    // Upsert overwrites in place.
    store.set_setting(setting_key::LAN_PORT, "9001").unwrap();
    assert_eq!(store.get_setting(setting_key::LAN_PORT).unwrap().as_deref(), Some("9001"));

    assert!(store.delete_setting(setting_key::LAN_ENABLED).unwrap());
    assert!(!store.get_bool_setting(setting_key::LAN_ENABLED).unwrap());
    assert!(!store.delete_setting(setting_key::LAN_ENABLED).unwrap(), "second delete is a no-op");
}

#[test]
fn phone_tokens_mint_resolve_list_revoke() {
    let store = Store::open_in_memory().unwrap();
    let id = store.add_phone_token("phone-tok-abc", Some("Matt's iPhone")).unwrap();

    // A live token resolves to its label and stamps last_seen.
    let row = store.resolve_phone_token("phone-tok-abc").unwrap().expect("live token resolves");
    assert_eq!(row.id, id);
    assert_eq!(row.name.as_deref(), Some("Matt's iPhone"));
    assert!(row.last_seen_at.is_some(), "resolve stamps last_seen_at");

    assert_eq!(store.list_phone_tokens(false).unwrap().len(), 1);

    // Revocation makes the token unresolvable and drops it from the live listing.
    assert!(store.revoke_phone_token(&id).unwrap());
    assert!(store.resolve_phone_token("phone-tok-abc").unwrap().is_none(), "revoked token is dead");
    assert_eq!(store.list_phone_tokens(false).unwrap().len(), 0, "not in live listing");
    assert_eq!(store.list_phone_tokens(true).unwrap().len(), 1, "still visible with history");
    assert!(!store.revoke_phone_token(&id).unwrap(), "second revoke is a no-op");
}

#[test]
fn round_schedules_default_off_and_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let project = store.add_project("C:/repos/sched", "Sched", "main", "pr").unwrap();

    // Default: both schedules NULL (rounds off), and the scheduler skips the project.
    assert_eq!(store.project_schedule(&project.id, false).unwrap(), None);
    assert_eq!(store.project_schedule(&project.id, true).unwrap(), None);
    assert!(store.list_project_schedules().unwrap().is_empty(), "all-default fleet is skipped");

    store.set_project_schedule(&project.id, false, Some(r#"{"enabled":true,"interval_minutes":60}"#)).unwrap();
    store.set_project_schedule(&project.id, true, Some(r#"{"enabled":true,"interval_minutes":10080}"#)).unwrap();
    assert!(store.project_schedule(&project.id, false).unwrap().unwrap().contains("60"));
    assert!(store.project_schedule(&project.id, true).unwrap().unwrap().contains("10080"));

    let scheduled = store.list_project_schedules().unwrap();
    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].0, project.id);

    // Clearing on empty string nulls the column back out.
    store.set_project_schedule(&project.id, false, None).unwrap();
    store.set_project_schedule(&project.id, true, Some("  ")).unwrap();
    assert_eq!(store.project_schedule(&project.id, false).unwrap(), None);
    assert_eq!(store.project_schedule(&project.id, true).unwrap(), None);
    assert!(store.list_project_schedules().unwrap().is_empty());
}
