//! The agent-side `dflow` CLI (`agent-cli.md`).
//!
//! A tiny cross-platform binary placed on PATH in every leased worktree. It is how a
//! worker agent (on any harness) reads its card, self-reports state, maintains the
//! board, and reads/writes project knowledge. Dispatch injects `DFLOW_TOKEN` (per-task
//! scoped), `DFLOW_CARD`, and `DFLOW_ENDPOINT`; outside a dispatched context the CLI
//! fails fast (exit 3). See `agent-cli.md` for the verbs, AXI rules, and exit codes.

mod client;
mod error;
mod format;

use std::collections::{HashMap, HashSet};
use std::io::Read;

use dflow_proto::{
    AgentContext, AgentContextResult, ArtifactRegister, ArtifactRegistered, CardCreate, CardCreated,
    CardMove, CardResult, CardUpdate, FeedbackPoll, FeedbackPollResult, FindingAdd, FindingAddResult,
    KnowAdd, KnowAddResult, KnowFind, KnowFindResult, KnowGet, KnowGetResult, KnowIndex,
    KnowIndexResult, NotifyForward, RoundDigest, RoundDigestResult, SelfReport, SelfReportResult,
    SetNote, Simple,
};

use client::Client;
use error::CliError;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // The codex notify bridge is fire-and-forget: it must never break codex, so it
    // swallows every failure and always exits 0 (`agent-cli.md` / notify-forward).
    if args.first().map(String::as_str) == Some("notify-forward") {
        notify_forward(&args[1..]);
        std::process::exit(0);
    }

    match run(&args) {
        Ok(output) => {
            print!("{output}");
            std::process::exit(0);
        }
        Err(err) => {
            err.emit();
            std::process::exit(err.code);
        }
    }
}

/// The per-invocation environment injected by dispatch (`agent-cli.md` / wiring).
struct Env {
    token: String,
    endpoint: String,
    card: Option<String>,
}

impl Env {
    /// Load the dispatched-session environment, failing fast (exit 3) when absent.
    fn load() -> Result<Env, CliError> {
        let token = non_empty_var("DFLOW_TOKEN").ok_or_else(|| {
            CliError::not_dispatched("not running inside a dispatched DapperFlow session (DFLOW_TOKEN unset)")
        })?;
        let endpoint = non_empty_var("DFLOW_ENDPOINT").ok_or_else(|| {
            CliError::not_dispatched("DFLOW_ENDPOINT is unset; dispatch injects the daemon endpoint")
        })?;
        Ok(Env { token, endpoint, card: non_empty_var("DFLOW_CARD") })
    }
}

fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Route a verb. `help` needs no daemon; everything else connects first.
fn run(args: &[String]) -> Result<String, CliError> {
    if args.first().map(String::as_str) == Some("help") {
        return Ok(help(args.get(1).map(String::as_str)));
    }
    let env = Env::load()?;
    let mut client = Client::connect(&env.endpoint, &env.token)?;
    dispatch(args, &env, &mut client)
}

/// Dispatch an authenticated verb to the daemon and format its response.
fn dispatch(args: &[String], env: &Env, client: &mut Client) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        None => cmd_context(client),
        Some("card") => cmd_card(&args[1..], env, client),
        Some("status") => cmd_status(&args[1..], client),
        Some("know") => cmd_know(&args[1..], client),
        Some("plan") => cmd_plan(&args[1..], client),
        Some("round") => cmd_round(&args[1..], client),
        Some("finding") => cmd_finding(&args[1..], client),
        Some(other) => Err(CliError::usage(format!(
            "unknown command `{other}` (try `dflow help`)"
        ))),
    }
}

// ---- verbs ----

/// Bare `dflow`: current card, state, next action.
fn cmd_context(client: &mut Client) -> Result<String, CliError> {
    let val = client.request("agent.context", AgentContext {})?;
    let res: AgentContextResult = Client::decode(val)?;
    Ok(format::render_context(&res))
}

/// `dflow card [--full]` and its `create|update|note|move` subcommands.
fn cmd_card(args: &[String], env: &Env, client: &mut Client) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        Some("create") => cmd_card_create(&args[1..], client),
        Some("update") => cmd_card_update(&args[1..], env, client),
        Some("note") => cmd_card_note(&args[1..], client),
        Some("move") => cmd_card_move(&args[1..], env, client),
        _ => {
            // Plain `dflow card [--full]`: the brief + acceptance + digest view.
            let (_pos, _vals, bools) = parse(args, &[]);
            let val = client.request("agent.context", AgentContext {})?;
            let res: AgentContextResult = Client::decode(val)?;
            Ok(format::render_card(&res, bools.contains("full")))
        }
    }
}

fn cmd_card_create(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (_pos, vals, _bools) = parse(args, &["title", "type", "brief", "fingerprint"]);
    let title = vals
        .get("title")
        .cloned()
        .ok_or_else(|| CliError::usage("`dflow card create` requires --title"))?;
    let brief = match vals.get("brief").map(String::as_str) {
        Some("-") => Some(read_stdin()?),
        Some(text) => Some(text.to_string()),
        None => None,
    };
    let req = CardCreate {
        title,
        card_type: vals.get("type").cloned().unwrap_or_else(|| "feature".into()),
        project_id: None,
        dial_recipe: None,
        brief,
        priority: None,
        lane: None,
        fingerprint: vals.get("fingerprint").cloned(),
    };
    let val = client.request("card.create", req)?;
    let res: CardCreated = Client::decode(val)?;
    Ok(format::render_card_created(&res))
}

fn cmd_card_update(args: &[String], env: &Env, client: &mut Client) -> Result<String, CliError> {
    let (pos, vals, _bools) = parse(args, &["title", "brief"]);
    let card_id = pos.first().cloned().or_else(|| env.card.clone()).ok_or_else(|| {
        CliError::usage("no card in scope; pass an id or run inside a carded session")
    })?;
    let brief = match vals.get("brief").map(String::as_str) {
        Some("-") => Some(read_stdin()?),
        Some(text) => Some(text.to_string()),
        None => None,
    };
    if !vals.contains_key("title") && brief.is_none() {
        return Err(CliError::usage("`dflow card update` needs --title and/or --brief"));
    }
    let req = CardUpdate {
        card_id,
        title: vals.get("title").cloned(),
        card_type: None,
        dial_recipe: None,
        brief,
        priority: None,
    };
    let val = client.request("card.update", req)?;
    let res: CardResult = Client::decode(val)?;
    Ok(format::render_card_result(&res, "update"))
}

fn cmd_card_note(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (pos, _vals, _bools) = parse(args, &[]);
    let note = pos.join(" ");
    if note.trim().is_empty() {
        return Err(CliError::usage("`dflow card note` needs the note text"));
    }
    let val = client.request("session.set_note", SetNote { note: note.clone() })?;
    let _: Simple = Client::decode(val)?;
    Ok(format::render_note_set(note.trim()))
}

fn cmd_card_move(args: &[String], env: &Env, client: &mut Client) -> Result<String, CliError> {
    let (pos, _vals, _bools) = parse(args, &[]);
    let (card_id, lane) = match pos.as_slice() {
        [lane] => (
            env.card.clone().ok_or_else(|| {
                CliError::usage("no card in scope; pass an id: `dflow card move <id> <lane>`")
            })?,
            lane.clone(),
        ),
        [id, lane] => (id.clone(), lane.clone()),
        _ => return Err(CliError::usage("usage: `dflow card move [<id>] <lane>`")),
    };
    let val = client.request("card.move", CardMove { card_id, column: lane })?;
    let res: CardResult = Client::decode(val)?;
    Ok(format::render_card_result(&res, "move"))
}

/// `dflow status <working|blocked|done> [note]`.
fn cmd_status(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let state = args
        .first()
        .cloned()
        .ok_or_else(|| CliError::usage("usage: `dflow status <working|blocked|done> [note]`"))?;
    let note = args[1..].join(" ");
    let note = if note.trim().is_empty() { None } else { Some(note.trim().to_string()) };
    if state == "blocked" && note.is_none() {
        return Err(CliError::usage("`dflow status blocked` requires a note explaining the block"));
    }
    let req = SelfReport { state, note };
    let val = client.request("session.self_report", req)?;
    let res: SelfReportResult = Client::decode(val)?;
    Ok(format::render_status(&res))
}

/// `dflow know [find|get|add]`.
fn cmd_know(args: &[String], client: &mut Client) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        Some("find") => cmd_know_find(&args[1..], client),
        Some("get") => cmd_know_get(&args[1..], client),
        Some("add") => cmd_know_add(&args[1..], client),
        None => {
            let val = client.request("know.index", KnowIndex { project_id: None })?;
            let res: KnowIndexResult = Client::decode(val)?;
            Ok(format::render_know_index(&res))
        }
        Some(other) => Err(CliError::usage(format!(
            "unknown `know` subcommand `{other}` (find|get|add)"
        ))),
    }
}

fn cmd_know_find(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (pos, vals, _bools) = parse(args, &["type"]);
    let query = pos.join(" ");
    if query.trim().is_empty() {
        return Err(CliError::usage("usage: `dflow know find <query> [--type <t>]`"));
    }
    let req = KnowFind { query, note_type: vals.get("type").cloned(), project_id: None };
    let val = client.request("know.find", req)?;
    let res: KnowFindResult = Client::decode(val)?;
    Ok(format::render_know_find(&res))
}

fn cmd_know_get(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (pos, _vals, bools) = parse(args, &[]);
    let id = pos
        .first()
        .cloned()
        .ok_or_else(|| CliError::usage("usage: `dflow know get <id> [--full]`"))?;
    let req = KnowGet { id: id.clone(), full: bools.contains("full"), project_id: None };
    let val = client.request("know.get", req)?;
    let res: KnowGetResult = Client::decode(val)?;
    Ok(format::render_know_get(&res, &id))
}

fn cmd_know_add(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (_pos, vals, bools) = parse(args, &["type", "title", "file", "tags"]);
    let note_type = vals
        .get("type")
        .cloned()
        .ok_or_else(|| CliError::usage("`dflow know add` requires --type"))?;
    let title = vals
        .get("title")
        .cloned()
        .ok_or_else(|| CliError::usage("`dflow know add` requires --title"))?;
    let body = if let Some(path) = vals.get("file") {
        std::fs::read_to_string(path)
            .map_err(|e| CliError::operational(format!("reading {path}: {e}"), "check the path"))?
    } else if bools.contains("stdin") {
        read_stdin()?
    } else {
        return Err(CliError::usage(
            "`dflow know add` needs the body via --stdin or --file <f>",
        ));
    };
    let tags = vals
        .get("tags")
        .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let req = KnowAdd { note_type, title, body, tags, project_id: None };
    let val = client.request("know.add", req)?;
    let res: KnowAddResult = Client::decode(val)?;
    Ok(format::render_know_add(&res))
}

/// `dflow plan [open <file.html> | poll]` (`agent-cli.md` / Plan Studio loop).
fn cmd_plan(args: &[String], client: &mut Client) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        Some("open") => cmd_plan_open(&args[1..], client),
        Some("poll") => cmd_plan_poll(&args[1..], client),
        _ => Err(CliError::usage(
            "usage: `dflow plan open <file.html>` | `dflow plan poll`",
        )),
    }
}

/// `dflow plan open <file.html>`: register a plan artifact for review. The file path is
/// resolved to absolute so the daemon (which runs elsewhere) can read it.
fn cmd_plan_open(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (pos, vals, _bools) = parse(args, &["kind", "title"]);
    let file = pos
        .first()
        .cloned()
        .ok_or_else(|| CliError::usage("usage: `dflow plan open <file.html>`"))?;
    let path = std::fs::canonicalize(&file)
        .map_err(|e| CliError::operational(format!("cannot open '{file}': {e}"), "check the path"))?;
    let req = ArtifactRegister {
        card_id: None,
        path: path.to_string_lossy().into_owned(),
        kind: Some(vals.get("kind").cloned().unwrap_or_else(|| "plan".into())),
        title: vals.get("title").cloned(),
    };
    let val = client.request("artifact.register", req)?;
    let res: ArtifactRegistered = Client::decode(val)?;
    Ok(format::render_artifact_registered(&res))
}

/// `dflow plan poll`: bounded long-poll for the human's feedback batch. Safe to re-run
/// forever; feedback is never lost.
fn cmd_plan_poll(_args: &[String], client: &mut Client) -> Result<String, CliError> {
    let val = client.request("artifact.feedback.poll", FeedbackPoll { artifact_id: None, wait: true })?;
    let res: FeedbackPollResult = Client::decode(val)?;
    Ok(format::render_plan_poll(&res))
}

/// `dflow round [digest]` (`product.md` / Concertmaster rounds). Only valid inside a
/// headless round session, where dispatch injects the round-scoped token.
fn cmd_round(args: &[String], client: &mut Client) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        Some("digest") => cmd_round_digest(&args[1..], client),
        _ => Err(CliError::usage(
            "usage: `dflow round digest --body <markdown> [--findings <n>]`",
        )),
    }
}

/// `dflow finding add` (`gate.md` / Adversarial review). A gate reviewer session files a
/// structured finding against its active gate run.
fn cmd_finding(args: &[String], client: &mut Client) -> Result<String, CliError> {
    match args.first().map(String::as_str) {
        Some("add") => cmd_finding_add(&args[1..], client),
        _ => Err(CliError::usage(
            "usage: `dflow finding add --severity <blocker|major|minor> --body <text> [--category <mechanical|intent>] [--evidence <ptr>]`",
        )),
    }
}

/// `dflow round digest --body <markdown> [--findings <n>]`: file the round's single
/// escalation digest. `--body -` reads the digest markdown from stdin.
fn cmd_round_digest(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (_pos, vals, _bools) = parse(args, &["body", "findings"]);
    let body = match vals.get("body").map(String::as_str) {
        Some("-") => read_stdin()?,
        Some(text) => text.to_string(),
        None => return Err(CliError::usage("`dflow round digest` requires --body -|<markdown>")),
    };
    if body.trim().is_empty() {
        return Err(CliError::usage("`dflow round digest` needs a non-empty --body"));
    }
    let findings = vals.get("findings").and_then(|f| f.parse::<u32>().ok());
    let req = RoundDigest { body, findings };
    let val = client.request("round.digest", req)?;
    let res: RoundDigestResult = Client::decode(val)?;
    let verb = if res.deduped { "updated" } else { "filed" };
    Ok(format!(
        "digest {verb} ({} findings) on round card {}\nnext: nothing - the round is escalation-only; exit when done\n",
        res.findings, res.round_card
    ))
}

fn cmd_finding_add(args: &[String], client: &mut Client) -> Result<String, CliError> {
    let (_pos, vals, _bools) = parse(args, &["severity", "body", "category", "evidence"]);
    let severity = vals
        .get("severity")
        .cloned()
        .ok_or_else(|| CliError::usage("`dflow finding add` requires --severity <blocker|major|minor>"))?;
    let body = match vals.get("body").map(String::as_str) {
        Some("-") => read_stdin()?,
        Some(text) => text.to_string(),
        None => return Err(CliError::usage("`dflow finding add` requires --body <text> (or --body - for stdin)")),
    };
    let req = FindingAdd {
        severity,
        body,
        category: vals.get("category").cloned(),
        evidence: vals.get("evidence").cloned(),
    };
    let val = client.request("finding.add", req)?;
    let res: FindingAddResult = Client::decode(val)?;
    Ok(format::render_finding_added(&res))
}

/// The codex notify bridge (hidden): forward the codex payload to the daemon over the
/// per-task token. Best-effort; any failure is silent so codex is never disrupted.
fn notify_forward(args: &[String]) {
    // codex appends the JSON payload as the final argument; fall back to stdin.
    let payload = args
        .iter()
        .rev()
        .find(|a| a.trim_start().starts_with('{'))
        .cloned()
        .or_else(|| {
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).ok().map(|_| buf)
        })
        .unwrap_or_default();
    if payload.trim().is_empty() {
        return;
    }
    let env = match Env::load() {
        Ok(e) => e,
        Err(_) => return,
    };
    if let Ok(mut client) = Client::connect(&env.endpoint, &env.token) {
        let _ = client.request("notify.forward", NotifyForward { payload });
    }
}

// ---- argument parsing ----

/// Split `args` into positionals, `--flag value` / `--flag=value` values, and boolean
/// `--flag` presences. `value_flags` names the flags that consume the next token.
fn parse(args: &[String], value_flags: &[&str]) -> (Vec<String>, HashMap<String, String>, HashSet<String>) {
    let mut positionals = Vec::new();
    let mut values = HashMap::new();
    let mut bools = HashSet::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(rest) = arg.strip_prefix("--") {
            if let Some((k, v)) = rest.split_once('=') {
                values.insert(k.to_string(), v.to_string());
            } else if value_flags.contains(&rest) && i + 1 < args.len() {
                values.insert(rest.to_string(), args[i + 1].clone());
                i += 1;
            } else {
                bools.insert(rest.to_string());
            }
        } else {
            positionals.push(arg.clone());
        }
        i += 1;
    }
    (positionals, values, bools)
}

/// Read all of stdin as a string.
fn read_stdin() -> Result<String, CliError> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| CliError::operational(format!("reading stdin: {e}"), "pipe the content in"))?;
    Ok(buf)
}

/// `dflow help [verb]`: a concise per-verb reference.
fn help(verb: Option<&str>) -> String {
    match verb {
        Some("card") => "\
dflow card [--full]                    show the brief, acceptance criteria, and memory digest
dflow card create --title <t> [--type <bug|feature|chore>] [--brief -|<text>] [--fingerprint <slug>]
dflow card update [<id>] [--title <t>] [--brief -|<text>]
dflow card note <text>                 set the board's live status note
dflow card move [<id>] <lane>          move a card (inbox|shaping|ready|performing|verifying|pr|done)
"
        .to_string(),
        Some("status") => "\
dflow status working [note]            you are actively working
dflow status blocked <note>            you need a human decision (note required; notifies the captain)
dflow status done [note]               the work is complete (a stage-advance request)
"
        .to_string(),
        Some("know") => "\
dflow know                             the digest and catalog counts at a glance
dflow know find <query> [--type <t>]   search titles, tags, and descriptions
dflow know get <id> [--full]           print one note
dflow know add --type <t> --title <t> [--stdin | --file <f>] [--tags a,b]   record a durable note
"
        .to_string(),
        Some("finding") => "\
dflow finding add --severity <blocker|major|minor> --body <text> [--category <mechanical|intent>] [--evidence <ptr>]
                                       file a structured gate finding (gate reviewer sessions only)

Use --category mechanical for safe-mechanical issues a fixer can apply automatically (lint,
formatting, dead imports, trivial test fixes) and intent for anything touching behavior, API
shape, or scope. Every finding needs a concrete failure scenario, not vibes.
"
        .to_string(),
        Some("plan") => "\
dflow plan open <file.html> [--title <t>]   register a self-contained plan artifact for review
dflow plan poll                             bounded ~4min long-poll for the human's feedback batch

The loop: write a self-contained plan.html, `dflow plan open plan.html`, then loop on
`dflow plan poll`. A poll returns queued feedback (revise in place and re-open), `pending`
(re-poll), or `ended`/`approved` with a next step. Feedback is never lost.
"
        .to_string(),
        Some("round") => "\
dflow round digest --body -|<markdown> [--findings <n>]   file the round's ONE escalation digest

Only valid inside a headless Concertmaster round session. A round is escalation-only:
read fleet/board/knowledge, then file at most one deduplicated Needs You digest (or none).
"
        .to_string(),
        _ => "\
dflow - the DapperFlow agent CLI. Report state and maintain the board as you work.

  dflow                          current card, state, and next action
  dflow card [--full]            the brief, acceptance criteria, and memory digest
  dflow card create|update|note|move   maintain the board (see `dflow help card`)
  dflow status <working|blocked|done> [note]   tier-1 lifecycle self-report
  dflow know [find|get|add]      project knowledge (see `dflow help know`)
  dflow plan [open|poll]         the Plan Studio review loop (see `dflow help plan`)
  dflow round digest --body <md> file a round's one escalation digest (round sessions only)
  dflow finding add ...          file a gate finding (gate reviewers; see `dflow help finding`)
  dflow help [verb]              this reference

Exit codes: 0 ok, 1 operational error, 2 usage error, 3 not in a dispatched session, 4 token revoked.
"
        .to_string(),
    }
}
