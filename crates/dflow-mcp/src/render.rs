//! Compact, token-frugal text renderers for tool results.
//!
//! Conventions (stated in the server instructions so the Concertmaster can rely
//! on them):
//!
//! - Every entity id appears as a bracketed token `[kind:ULID]` - `[card:...]`,
//!   `[session:...]`, `[project:...]`, `[needs_you:...]`, `[note:...]`. The ids
//!   are stable daemon ids, so the UI can turn every mention into a one-click
//!   deep link (`product.md` / the one-click invariant extends to the
//!   Concertmaster's mouth).
//! - One line per entity; counts up front; free text quoted and truncated.

use std::collections::HashMap;

use dflow_proto::{Card, KnowFindResult, KnowGetResult, KnowIndexResult, NeedsYouItem, Project,
    SessionSummary, StyledSnapshot};

/// Cap for inline free text (titles, notes) so outputs stay frugal.
const TEXT_CAP: usize = 100;

/// Render a duration in compact form: `45s`, `12m`, `3h05m`, `2d04h`.
pub fn age(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else {
        format!("{}d{:02}h", s / 86_400, (s % 86_400) / 3600)
    }
}

/// Collapse whitespace/newlines and cap length for one-line inline text.
fn inline(text: &str) -> String {
    let joined: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > TEXT_CAP {
        let cut: String = joined.chars().take(TEXT_CAP).collect();
        format!("{cut}...")
    } else {
        joined
    }
}

/// Milliseconds elapsed since an epoch-ms instant (clamped at zero).
fn since_ms(epoch_ms: i64) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    (now - epoch_ms).max(0) as u64
}

/// One fleet-table session line.
fn session_line(s: &SessionSummary) -> String {
    let card = s
        .card_id
        .as_deref()
        .map(|c| format!("[card:{c}]"))
        .unwrap_or_else(|| "-".into());
    let project = s.project_name.as_deref().unwrap_or("-");
    let agent = s.agent.as_deref().unwrap_or(&s.harness);
    let note = s
        .status_note
        .as_deref()
        .map(|n| format!(" note=\"{}\"", inline(n)))
        .unwrap_or_default();
    let alive = if s.alive { "" } else { " (no live pty)" };
    format!(
        "[session:{}] {agent} state={} up={} card={card} project={project}{note}{alive}",
        s.session_id,
        s.state,
        age(s.elapsed_ms),
    )
}

/// One Needs You line; `titles` maps card id -> title for context.
fn needs_you_line(item: &NeedsYouItem, titles: &HashMap<String, String>) -> String {
    let title = titles
        .get(&item.card_id)
        .map(|t| format!(" \"{}\"", inline(t)))
        .unwrap_or_default();
    format!(
        "[needs_you:{}] kind={} score={} age={} card=[card:{}]{title}",
        item.id,
        item.kind,
        item.score,
        age(since_ms(item.raised_at)),
        item.card_id,
    )
}

/// The `fleet_status` result: sessions plus the Needs You queue.
pub fn fleet(
    sessions: &[SessionSummary],
    needs_you: &[NeedsYouItem],
    titles: &HashMap<String, String>,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("sessions: {}\n", sessions.len()));
    for s in sessions {
        out.push_str(&session_line(s));
        out.push('\n');
    }
    out.push_str(&format!("needs_you: {}\n", needs_you.len()));
    for item in needs_you {
        out.push_str(&needs_you_line(item, titles));
        out.push('\n');
    }
    out
}

/// The `needs_you_list` result: the queue alone, highest score first.
pub fn needs_you(items: &[NeedsYouItem], titles: &HashMap<String, String>) -> String {
    if items.is_empty() {
        return "needs_you: 0 (nothing is waiting on the human right now)\n".into();
    }
    let mut out = format!("needs_you: {}\n", items.len());
    for item in items {
        out.push_str(&needs_you_line(item, titles));
        out.push('\n');
    }
    out
}

/// The `project_list` result.
pub fn projects(projects: &[Project]) -> String {
    if projects.is_empty() {
        return "projects: 0 (register one from the app: Projects > Add)\n".into();
    }
    let mut out = format!("projects: {}\n", projects.len());
    for p in projects {
        out.push_str(&format!(
            "[project:{}] {} mode={} branch={} path={}\n",
            p.id, p.name, p.mode, p.default_branch, p.path
        ));
    }
    out
}

/// One board-card line.
pub fn card_line(card: &Card) -> String {
    let project = card
        .project_id
        .as_deref()
        .map(|p| format!("[project:{p}]"))
        .unwrap_or_else(|| "-".into());
    format!(
        "[card:{}] lane={} type={} prio={} project={project} \"{}\"",
        card.id,
        card.lane,
        card.card_type,
        card.priority,
        inline(&card.title),
    )
}

/// The `board_query` result.
pub fn cards(cards: &[Card]) -> String {
    if cards.is_empty() {
        return "cards: 0 (no cards match the filter)\n".into();
    }
    let mut out = format!("cards: {}\n", cards.len());
    for c in cards {
        out.push_str(&card_line(c));
        out.push('\n');
    }
    out
}

/// The `knowledge_digest` result.
pub fn know_index(res: &KnowIndexResult) -> String {
    let mut out = String::new();
    if let Some(name) = &res.project_name {
        out.push_str(&format!("project: {name}\n"));
    }
    match &res.digest {
        Some(digest) if !digest.trim().is_empty() => {
            out.push_str(&format!("digest ({} lines):\n{}\n", res.digest_lines, digest.trim_end()));
        }
        _ => out.push_str("digest: (empty)\n"),
    }
    out.push_str(&format!("notes: {}", res.total_notes));
    if !res.catalog.is_empty() {
        let groups: Vec<String> =
            res.catalog.iter().map(|g| format!("{} {}", g.note_type, g.count)).collect();
        out.push_str(&format!(" ({})", groups.join(", ")));
    }
    out.push('\n');
    out
}

/// The `knowledge_find` result.
pub fn know_find(res: &KnowFindResult) -> String {
    if res.notes.is_empty() {
        return "notes: 0 (no knowledge notes match)\n".into();
    }
    let mut out = format!("notes: {}\n", res.notes.len());
    for n in &res.notes {
        out.push_str(&format!("[note:{}] type={} {}\n", n.id, n.note_type, inline(&n.description)));
    }
    out
}

/// The `knowledge_get` result.
pub fn know_get(res: &KnowGetResult, id: &str) -> String {
    match &res.note {
        None => format!("no knowledge note with id '{id}' (use knowledge_find to search)\n"),
        Some(note) => {
            let title = note.title.as_deref().unwrap_or(&note.id);
            let mut out = format!("[note:{}] type={} \"{}\"\n{}\n", note.id, note.note_type, title, note.body.trim_end());
            if note.truncated {
                out.push_str(&format!(
                    "(truncated: {} total lines; call knowledge_get with full=true for the rest)\n",
                    note.total_lines
                ));
            }
            out
        }
    }
}

/// Flatten a styled screen snapshot to plain text, keeping at most the last
/// `max_lines` non-trailing-blank lines (`session_peek`).
pub fn snapshot_text(snapshot: &StyledSnapshot, max_lines: usize) -> String {
    let mut lines: Vec<String> = snapshot
        .lines
        .iter()
        .map(|runs| {
            let mut line = String::new();
            for run in runs {
                line.push_str(&run.text);
            }
            line.trim_end().to_string()
        })
        .collect();
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use dflow_proto::StyledRun;

    fn run(text: &str) -> StyledRun {
        StyledRun {
            text: text.into(),
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }

    #[test]
    fn age_is_compact() {
        assert_eq!(age(45_000), "45s");
        assert_eq!(age(12 * 60_000), "12m");
        assert_eq!(age(3 * 3_600_000 + 5 * 60_000), "3h05m");
        assert_eq!(age(2 * 86_400_000 + 4 * 3_600_000), "2d04h");
    }

    #[test]
    fn inline_collapses_and_caps() {
        assert_eq!(inline("a\nb\t c"), "a b c");
        let long = "x".repeat(200);
        let out = inline(&long);
        assert!(out.ends_with("...") && out.chars().count() == TEXT_CAP + 3);
    }

    #[test]
    fn snapshot_flattens_and_bounds() {
        let snap = StyledSnapshot {
            cols: 10,
            rows: 5,
            lines: vec![
                vec![run("one")],
                vec![run("two "), run("halves  ")],
                vec![run("three")],
                vec![],
                vec![run("")],
            ],
        };
        assert_eq!(snapshot_text(&snap, 10), "one\ntwo halves\nthree");
        assert_eq!(snapshot_text(&snap, 2), "two halves\nthree");
    }

    #[test]
    fn card_line_carries_stable_id_tokens() {
        let card = Card {
            id: "01CARD".into(),
            project_id: Some("01PROJ".into()),
            card_type: "bug".into(),
            title: "Fix login".into(),
            lane: "inbox".into(),
            dial_recipe: None,
            priority: 2,
            brief: None,
            origin_kind: "manual".into(),
            origin_ref: None,
            created_at: 0,
            updated_at: 0,
        };
        let line = card_line(&card);
        assert!(line.contains("[card:01CARD]"));
        assert!(line.contains("[project:01PROJ]"));
        assert!(line.contains("lane=inbox"));
    }
}
