//! AXI output formatting (`agent-cli.md` / Design rules): compact aligned tables,
//! pre-computed aggregates, definitive empty states, a `next:` line on every response,
//! truncation with size hints. Every function here is pure (structured input ->
//! string) so the output contract is snapshot-testable without a daemon.

use dflow_proto::{
    AgentContextResult, ArtifactRegistered, Card, CardCreated, CardResult, FeedbackItem,
    FeedbackPollResult, FindingAddResult, KnowAddResult, KnowFindResult, KnowGetResult,
    KnowIndexResult, LayoutWarning, SelfReportResult,
};

/// Bare `dflow`: the current card, state, and next action (content-first default).
pub fn render_context(res: &AgentContextResult) -> String {
    let mut out = String::new();
    match &res.card {
        Some(card) => {
            out.push_str(&card_headline(card, res.session_state.as_deref(), res.project_name.as_deref()));
            out.push('\n');
            if let Some(note) = res.status_note.as_deref().filter(|n| !n.is_empty()) {
                out.push_str(&format!("note: {note}\n"));
            }
            out.push_str(&format!("next: {}\n", context_next(res.session_state.as_deref())));
        }
        None => {
            out.push_str("no card assigned (cardless session)\n");
            out.push_str(
                "next: `dflow card create --title \"...\" --type <bug|feature|chore>` to file work as you discover it\n",
            );
        }
    }
    out
}

/// `dflow card`: brief, acceptance criteria, and the project memory digest.
pub fn render_card(res: &AgentContextResult, full: bool) -> String {
    let card = match &res.card {
        Some(c) => c,
        None => {
            return "no card assigned (cardless session)\n\
                next: `dflow card create` to file work, or ask the captain to dispatch one\n"
                .to_string()
        }
    };
    let mut out = String::new();
    out.push_str(&card_headline(card, None, res.project_name.as_deref()));
    out.push('\n');

    if res.acceptance.is_empty() {
        out.push_str("acceptance: none recorded\n");
    } else {
        out.push_str(&format!("acceptance ({}):\n", res.acceptance.len()));
        for (i, item) in res.acceptance.iter().enumerate() {
            out.push_str(&format!("  {} {}\n", i + 1, item));
        }
    }

    match res.digest.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => out.push_str(&format!("memory digest: {}\n", oneline(d, 200))),
        None => out.push_str("memory digest: none recorded\n"),
    }

    if full {
        if let Some(brief) = card.brief.as_deref().filter(|b| !b.trim().is_empty()) {
            out.push_str("brief:\n");
            out.push_str(brief.trim_end());
            out.push('\n');
        } else {
            out.push_str("brief: (none)\n");
        }
        out.push_str("next: run `dflow status working` and begin\n");
    } else {
        out.push_str("next: run `dflow status working` and begin; `dflow card --full` for the whole brief\n");
    }
    out
}

/// `dflow status <state> [note]`: the recorded state and next step.
pub fn render_status(res: &SelfReportResult) -> String {
    let mut out = String::new();
    match res.recorded.as_str() {
        "blocked" => {
            out.push_str("recorded: blocked\n");
            out.push_str(
                "next: stop working; the captain has been notified and will respond via steer or plan feedback\n",
            );
        }
        "done" => {
            if res.advanced {
                out.push_str("recorded: done (stage advanced)\n");
                // Prefer the daemon's recipe-aware hint (composed from the dispatch
                // recipe's stage list) over the generic line.
                match res.next.as_deref() {
                    Some(next) => out.push_str(&format!("next: {next}\n")),
                    None => out.push_str(
                        "next: the implement stage is complete; verification takes over from here\n",
                    ),
                }
            } else {
                let reason = res.blocked_reason.as_deref().unwrap_or("a recipe condition is unmet");
                out.push_str("recorded: done (request pending)\n");
                out.push_str(&format!("next: not advanced yet - {reason}; a Needs You item was raised for the human\n"));
            }
        }
        other => {
            out.push_str(&format!("recorded: {other}\n"));
            out.push_str(
                "next: continue; `dflow status blocked \"<why>\"` if you need a decision, `dflow status done` when complete\n",
            );
        }
    }
    out
}

/// `dflow card create`: the new card and what to do next.
pub fn render_card_created(res: &CardCreated) -> String {
    let c = &res.card;
    let mut out = String::new();
    out.push_str(&format!(
        "created card {}  {}  \"{}\"  lane:{}\n",
        c.id, c.card_type, c.title, c.lane
    ));
    out.push_str(
        "next: `dflow card move performing` when you start it, or keep filing follow-ups as you find them\n",
    );
    out
}

/// `dflow card update` / `dflow card move`: the updated card.
pub fn render_card_result(res: &CardResult, action: &str) -> String {
    let c = &res.card;
    let mut out = String::new();
    match action {
        "move" => {
            out.push_str(&format!("moved card {} -> {}\n", c.id, c.lane));
            out.push_str("next: run `dflow status working` if you have not already\n");
        }
        _ => {
            out.push_str(&format!("updated card {}  \"{}\"\n", c.id, c.title));
            out.push_str("next: `dflow card note \"<one line>\"` to keep the board's status line current\n");
        }
    }
    out
}

/// `dflow card note <text>`: confirm the session-strip note.
pub fn render_note_set(note: &str) -> String {
    format!(
        "note set: {note}\nnext: keep it current as your focus changes; self-report with `dflow status`\n"
    )
}

/// `dflow finding add`: confirm the filed finding (`gate.md` / Adversarial review).
pub fn render_finding_added(res: &FindingAddResult) -> String {
    format!(
        "finding filed: [{severity}/{category}] {id}\nnext: file more findings, or exit when your review is complete\n",
        severity = res.severity,
        category = res.category,
        id = res.finding_id,
    )
}

/// `dflow know`: the digest and catalog counts at a glance.
pub fn render_know_index(res: &KnowIndexResult) -> String {
    let mut out = String::new();
    let project = res.project_name.as_deref().unwrap_or("project");
    match res.digest.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => out.push_str(&format!(
            "digest ({project}, {} lines): {}\n",
            res.digest_lines,
            oneline(d, 160)
        )),
        None => out.push_str(&format!("digest ({project}): none yet\n")),
    }
    if res.catalog.is_empty() {
        out.push_str("catalog: no notes yet\n");
        out.push_str(
            "next: record durable facts with `dflow know add --type <t> --title \"...\" --stdin`\n",
        );
    } else {
        let parts: Vec<String> = res
            .catalog
            .iter()
            .map(|g| format!("{} {}", g.count, g.note_type))
            .collect();
        out.push_str(&format!("catalog: {}\n", parts.join(", ")));
        out.push_str(
            "next: `dflow know find <query>` before re-deriving anything; `dflow know add` when you learn something durable\n",
        );
    }
    out
}

/// `dflow know find <query>`: a compact aligned hit table.
pub fn render_know_find(res: &KnowFindResult) -> String {
    if res.notes.is_empty() {
        return "no notes match\n\
            next: if you derive the answer, record it: `dflow know add --type <t> --title \"...\" --stdin`\n"
            .to_string();
    }
    let id_w = res.notes.iter().map(|n| n.id.len()).max().unwrap_or(0);
    let type_w = res.notes.iter().map(|n| n.note_type.len()).max().unwrap_or(0);
    let mut out = format!("{} note{}:\n", res.notes.len(), plural(res.notes.len()));
    for n in &res.notes {
        let desc = oneline(&n.description, 80);
        out.push_str(&format!(
            "  {:id_w$}  {:type_w$}  {}\n",
            n.id,
            n.note_type,
            desc,
            id_w = id_w,
            type_w = type_w
        ));
    }
    let first = &res.notes[0].id;
    out.push_str(&format!("next: `dflow know get {first}`\n"));
    out
}

/// `dflow know get <id>`: one note, truncated with a size hint unless `--full`.
pub fn render_know_get(res: &KnowGetResult, id: &str) -> String {
    let note = match &res.note {
        Some(n) => n,
        None => {
            return format!(
                "no note with id `{id}`\nnext: `dflow know find <query>` to locate the right id\n"
            )
        }
    };
    let mut out = String::new();
    let title = note.title.as_deref().unwrap_or("");
    out.push_str(&format!("{}  {}  \"{}\"\n", note.id, note.note_type, title));
    out.push_str(note.body.trim_end());
    out.push('\n');
    if note.truncated {
        let shown = note.body.lines().count() as u32;
        let more = note.total_lines.saturating_sub(shown);
        out.push_str(&format!(
            "... ({more} more lines of {}; `dflow know get {} --full` for all)\n",
            note.total_lines, note.id
        ));
        out.push_str("next: read `--full` if you need the rest, then apply it\n");
    } else {
        out.push_str("next: apply it; record any correction with `dflow know add`\n");
    }
    out
}

/// `dflow know add`: confirm the recorded note.
pub fn render_know_add(res: &KnowAddResult) -> String {
    let verb = if res.created { "new" } else { "updated" };
    format!(
        "recorded: {} ({verb})  {}\nnext: it is indexed and in the catalog; the captain commits it alongside your changes\n",
        res.id, res.path
    )
}

/// `dflow plan open`: confirm the registered artifact and point at the poll.
pub fn render_artifact_registered(res: &ArtifactRegistered) -> String {
    let a = &res.artifact;
    let verb = if res.revised { "revised" } else { "opened" };
    let mut out = format!("{verb} plan artifact {}  round:{}  status:{}\n", a.id, a.round, a.status);
    out.push_str(&format!("review: {}\n", res.review_hint));
    out.push_str(
        "next: run `dflow plan poll` now and keep it in the FOREGROUND - it blocks until the \
         human responds; do not background it or end your session\n",
    );
    out
}

/// `dflow plan poll`: the queued feedback batch, `pending`, or `ended`, plus the layout
/// audit line and a `next:` step (`agent-cli.md` / example outputs).
pub fn render_plan_poll(res: &FeedbackPollResult) -> String {
    let mut out = String::new();
    if res.ended {
        let how = if res.approved { "approved" } else { "ended" };
        out.push_str(&format!("review {how} (round {})\n", res.round));
        if !res.items.is_empty() {
            out.push_str(&format!("final feedback ({} item{}):\n", res.items.len(), plural(res.items.len())));
            for (i, item) in res.items.iter().enumerate() {
                out.push_str(&format!("  {} {}\n", i + 1, render_feedback_item(item)));
            }
        }
    } else if res.pending {
        out.push_str(&format!("no feedback queued yet (round {})\n", res.round));
    } else if res.items.is_empty() {
        out.push_str(&format!("no feedback in this batch (round {})\n", res.round));
    } else {
        out.push_str(&format!("feedback ({} item{}, round {}):\n", res.items.len(), plural(res.items.len()), res.round));
        for (i, item) in res.items.iter().enumerate() {
            out.push_str(&format!("  {} {}\n", i + 1, render_feedback_item(item)));
        }
    }
    out.push_str(&format!("layout: {}\n", render_layout(&res.layout_warnings)));
    out.push_str(&format!("next: {}\n", res.next_step));
    out
}

/// One feedback item as a compact line (`agent-cli.md` example).
fn render_feedback_item(item: &FeedbackItem) -> String {
    let body = item.body.as_deref().unwrap_or("");
    match item.kind.as_str() {
        "text_range" => {
            let quote = item.anchor.as_ref().map(|a| oneline(&a.quote, 60)).unwrap_or_default();
            let status = item.status.as_deref().map(|s| format!(" [{s}]")).unwrap_or_default();
            format!("[text-range]{status} \"{quote}\" > {body}")
        }
        "control" => {
            let key = item.question_key.as_deref().unwrap_or("?");
            let value = item.value.as_ref().map(render_json_value).unwrap_or_default();
            format!("[control q:{key}] user selected: {value}")
        }
        "diagram_node" => {
            let diagram = item.diagram.as_deref().unwrap_or("?");
            let node = item.node.as_deref().unwrap_or("?");
            format!("[diagram {diagram}/{node}] > {body}")
        }
        "action" => {
            let action = item.action.as_deref().unwrap_or("?");
            if body.is_empty() {
                format!("[action {action}]")
            } else {
                format!("[action {action}] > {body}")
            }
        }
        "chat" => format!("[chat] {body}"),
        other => format!("[{other}] {body}"),
    }
}

/// The layout-audit line: `clean` or a count with the finding kinds.
fn render_layout(warnings: &[LayoutWarning]) -> String {
    if warnings.is_empty() {
        return "clean".to_string();
    }
    let errors = warnings.iter().filter(|w| w.severity == "error").count();
    let kinds: Vec<&str> = warnings.iter().map(|w| w.kind.as_str()).collect();
    format!(
        "{} finding{} ({} error): {}",
        warnings.len(),
        plural(warnings.len()),
        errors,
        kinds.join(", ")
    )
}

/// Render a JSON control value compactly (string without quotes, else its JSON form).
fn render_json_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---- shared helpers ----

/// A one-line card headline: `card <id>  <type>  "<title>"  [dial:<r>]  [state:<s>]
/// [project:<name>]`.
fn card_headline(card: &Card, state: Option<&str>, project: Option<&str>) -> String {
    let mut line = format!("card {}  {}  \"{}\"", card.id, card.card_type, card.title);
    if let Some(dial) = card.dial_recipe.as_deref().filter(|d| !d.is_empty()) {
        line.push_str(&format!("  dial:{dial}"));
    }
    if let Some(s) = state {
        line.push_str(&format!("  state:{s}"));
    }
    if let Some(p) = project {
        line.push_str(&format!("  project:{p}"));
    }
    line
}

/// The `next:` hint for bare `dflow`, tuned to the session state.
fn context_next(state: Option<&str>) -> &'static str {
    match state.unwrap_or("") {
        "working" => "continue; `dflow status blocked \"<why>\"` if you need a decision, `dflow status done` when complete",
        "blocked" | "needs_input" => "you are blocked; wait for the captain, or `dflow status working` to resume",
        "done" => "this session is done",
        _ => "run `dflow status working` and begin",
    }
}

/// Collapse text to one line and truncate to `max` chars with an ellipsis.
fn oneline(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > max {
        let head: String = joined.chars().take(max.saturating_sub(3)).collect();
        format!("{head}...")
    } else {
        joined
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests;
