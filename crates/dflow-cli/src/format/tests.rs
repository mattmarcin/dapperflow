//! Snapshot tests for the AXI output contract (`agent-cli.md`). The formatters are
//! pure, so these assert exact text without a daemon.

use super::*;
use dflow_proto::{KnowCatalogGroup, KnowNote, KnowNoteHit};

fn card(id: &str, ctype: &str, title: &str, lane: &str) -> Card {
    Card {
        id: id.into(),
        project_id: Some("PROJ".into()),
        card_type: ctype.into(),
        title: title.into(),
        lane: lane.into(),
        dial_recipe: None,
        priority: 0,
        brief: None,
        origin_kind: "manual".into(),
        origin_ref: None,
        created_at: 0,
        updated_at: 0,
    }
}

fn ctx(card: Option<Card>) -> AgentContextResult {
    AgentContextResult {
        card,
        project_name: Some("acme-web".into()),
        session_id: Some("SESS".into()),
        session_state: Some("working".into()),
        status_note: None,
        acceptance: Vec::new(),
        digest: None,
        knowledge_notes: 0,
    }
}

#[test]
fn context_with_card_ends_in_next() {
    let out = render_context(&ctx(Some(card("01JX8F", "feature", "Add dark mode toggle", "performing"))));
    assert_eq!(
        out,
        "card 01JX8F  feature  \"Add dark mode toggle\"  state:working  project:acme-web\n\
         next: continue; `dflow status blocked \"<why>\"` if you need a decision, `dflow status done` when complete\n"
    );
}

#[test]
fn context_cardless_has_definitive_empty_state() {
    let out = render_context(&ctx(None));
    assert!(out.starts_with("no card assigned (cardless session)\n"));
    assert!(out.trim_end().ends_with("as you discover it"));
    assert!(out.contains("next: "));
}

#[test]
fn card_view_lists_acceptance_and_digest() {
    let mut c = ctx(Some(card("01JX8F", "feature", "Add dark mode toggle", "performing")));
    c.session_state = None;
    c.acceptance = vec![
        "toggle persists across sessions".into(),
        "respects system preference by default".into(),
        "no flash of wrong theme on load".into(),
    ];
    c.digest = Some("acme-web uses tailwind + next-themes is NOT installed; check docs/theming.md".into());
    let out = render_card(&c, false);
    assert_eq!(
        out,
        "card 01JX8F  feature  \"Add dark mode toggle\"  project:acme-web\n\
         acceptance (3):\n\
         \x20 1 toggle persists across sessions\n\
         \x20 2 respects system preference by default\n\
         \x20 3 no flash of wrong theme on load\n\
         memory digest: acme-web uses tailwind + next-themes is NOT installed; check docs/theming.md\n\
         next: run `dflow status working` and begin; `dflow card --full` for the whole brief\n"
    );
}

#[test]
fn card_view_definitive_empty_states() {
    let out = render_card(&ctx(Some(card("01JX8F", "bug", "Fix it", "inbox"))), false);
    assert!(out.contains("acceptance: none recorded\n"));
    assert!(out.contains("memory digest: none recorded\n"));
}

#[test]
fn status_blocked_matches_spec_example() {
    let res = SelfReportResult {
        recorded: "blocked".into(),
        advanced: false,
        note_set: true,
        blocked_reason: None,
        next: None,
    };
    assert_eq!(
        render_status(&res),
        "recorded: blocked\n\
         next: stop working; the captain has been notified and will respond via steer or plan feedback\n"
    );
}

#[test]
fn status_done_advanced() {
    let res = SelfReportResult {
        recorded: "done".into(),
        advanced: true,
        note_set: false,
        blocked_reason: None,
        next: None,
    };
    let out = render_status(&res);
    assert!(out.starts_with("recorded: done (stage advanced)\n"));
    assert!(out.contains("next: "));
}

#[test]
fn status_done_prefers_daemon_recipe_hint() {
    // The daemon composes a recipe-aware `next` from the dispatch recipe's stage list
    // (`agent-cli.md` / Stage advancement arbitration); the CLI must surface it.
    let res = SelfReportResult {
        recorded: "done".into(),
        advanced: true,
        note_set: false,
        blocked_reason: None,
        next: Some("recipe audit v1 ends at implement; the card completes from here".into()),
    };
    let out = render_status(&res);
    assert!(out.contains("next: recipe audit v1 ends at implement"), "got: {out}");
}

#[test]
fn status_working_next_line() {
    let res = SelfReportResult {
        recorded: "working".into(),
        advanced: false,
        note_set: true,
        blocked_reason: None,
        next: None,
    };
    let out = render_status(&res);
    assert_eq!(out.lines().next().unwrap(), "recorded: working");
    assert!(out.trim_end().ends_with("when complete"));
}

#[test]
fn card_created_shows_id_and_lane() {
    let res = CardCreated { card_id: "01NEW".into(), card: card("01NEW", "bug", "Logout race", "inbox"), dedupe: None };
    let out = render_card_created(&res);
    assert_eq!(out.lines().next().unwrap(), "created card 01NEW  bug  \"Logout race\"  lane:inbox");
    assert!(out.contains("next: "));
}

#[test]
fn card_move_shows_transition() {
    let res = CardResult { card: card("01NEW", "bug", "Logout race", "performing") };
    let out = render_card_result(&res, "move");
    assert_eq!(out.lines().next().unwrap(), "moved card 01NEW -> performing");
}

#[test]
fn know_index_aggregates_catalog() {
    let res = KnowIndexResult {
        project_name: Some("acme-web".into()),
        digest: Some("tailwind, next-themes NOT installed; accounts soft-delete 90d".into()),
        digest_lines: 12,
        catalog: vec![
            KnowCatalogGroup { note_type: "decisions".into(), count: 4 },
            KnowCatalogGroup { note_type: "gotchas".into(), count: 2 },
        ],
        total_notes: 6,
    };
    let out = render_know_index(&res);
    assert!(out.starts_with("digest (acme-web, 12 lines): tailwind, next-themes NOT installed"));
    assert!(out.contains("catalog: 4 decisions, 2 gotchas\n"));
    assert!(out.contains("next: `dflow know find <query>` before re-deriving anything"));
}

#[test]
fn know_index_empty_state() {
    let res = KnowIndexResult { project_name: Some("acme".into()), digest: None, digest_lines: 0, catalog: vec![], total_notes: 0 };
    let out = render_know_index(&res);
    assert!(out.contains("digest (acme): none yet\n"));
    assert!(out.contains("catalog: no notes yet\n"));
}

#[test]
fn know_find_aligns_and_points_at_get() {
    let res = KnowFindResult {
        notes: vec![
            KnowNoteHit {
                id: "decisions/soft-delete-accounts".into(),
                note_type: "decision".into(),
                description: "accounts soft-delete, 90-day purge".into(),
            },
            KnowNoteHit {
                id: "runbooks/purge-job".into(),
                note_type: "runbook".into(),
                description: "manual purge-job runbook".into(),
            },
        ],
    };
    let out = render_know_find(&res);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "2 notes:");
    // Columns are aligned to the widest id.
    assert_eq!(lines[1], "  decisions/soft-delete-accounts  decision  accounts soft-delete, 90-day purge");
    assert_eq!(lines[2], "  runbooks/purge-job              runbook   manual purge-job runbook");
    assert_eq!(lines[3], "next: `dflow know get decisions/soft-delete-accounts`");
}

#[test]
fn know_find_empty_is_definitive() {
    let out = render_know_find(&KnowFindResult { notes: vec![] });
    assert!(out.starts_with("no notes match\n"));
    assert!(out.contains("next: "));
}

#[test]
fn know_get_truncates_with_size_hint() {
    let res = KnowGetResult {
        note: Some(KnowNote {
            id: "gotchas/x".into(),
            note_type: "gotcha".into(),
            title: Some("A gotcha".into()),
            body: "line1\nline2".into(),
            truncated: true,
            total_lines: 50,
        }),
    };
    let out = render_know_get(&res, "gotchas/x");
    assert!(out.starts_with("gotchas/x  gotcha  \"A gotcha\"\n"));
    assert!(out.contains("48 more lines of 50; `dflow know get gotchas/x --full` for all"));
}

#[test]
fn know_get_missing_id() {
    let out = render_know_get(&KnowGetResult { note: None }, "nope/x");
    assert!(out.starts_with("no note with id `nope/x`\n"));
}

#[test]
fn know_add_created_vs_updated() {
    let new = render_know_add(&KnowAddResult { id: "gotchas/x".into(), path: "gotchas/x.md".into(), created: true });
    assert!(new.starts_with("recorded: gotchas/x (new)  gotchas/x.md\n"));
    let upd = render_know_add(&KnowAddResult { id: "gotchas/x".into(), path: "gotchas/x.md".into(), created: false });
    assert!(upd.starts_with("recorded: gotchas/x (updated)  gotchas/x.md\n"));
}

#[test]
fn every_render_ends_with_a_next_line() {
    // AXI rule 6: every response ends with a `next:` line.
    let samples = vec![
        render_context(&ctx(Some(card("1", "bug", "t", "inbox")))),
        render_context(&ctx(None)),
        render_card(&ctx(Some(card("1", "bug", "t", "inbox"))), false),
        render_status(&SelfReportResult {
            recorded: "working".into(),
            advanced: false,
            note_set: false,
            blocked_reason: None,
            next: None,
        }),
        render_note_set("a note"),
        render_know_index(&KnowIndexResult { project_name: None, digest: None, digest_lines: 0, catalog: vec![], total_notes: 0 }),
        render_know_find(&KnowFindResult { notes: vec![] }),
    ];
    for s in samples {
        let last = s.trim_end().lines().last().unwrap();
        assert!(last.starts_with("next:"), "output did not end with a next line: {s:?}");
    }
}
