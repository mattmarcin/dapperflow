//! The verification gate engine (`gate.md` / Pipeline; `roadmap.md` M5.3).
//!
//! `gate.run` on a card leases an isolated gate-class worktree (never the authoring
//! worktree - the author's lease still holds it, so the pool hands out a different
//! slot), checks out the commit under test with warm caches intact, then runs the
//! pipeline: checks -> adversarial review (a reviewer session on a DIFFERENT harness
//! than the author, filing findings via `dflow finding add`) -> autofix (safe-mechanical
//! findings applied by a fixer session, re-checked) -> escalation (intent-touching
//! findings become Needs You `gate_finding` items) -> ship (stage 5). Every step appends
//! a `card_events` row with an evidence pointer, so the timeline shows exactly why a
//! branch was (or was not) allowed out.
//!
//! The pipeline runs on a background thread because it blocks on git, check commands, and
//! reviewer/fixer PTY sessions; `gate.run` returns the run id immediately and progress
//! streams over `event.subscribe`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dflow_core::github::{Gh, MergeMethod, PrCreate};
use dflow_core::recipe::{GateStrictness, ShipTarget, Stage};
use dflow_core::{
    category, gate_status, gate_step, harness, resolution, severity, FindingRow, GateRunRow,
    NewGateRun, Recipe, SessionSpec,
};
use dflow_proto::{
    FindingAdd, FindingAddResult, FindingInfo, FindingResult, GateMerge, GateMergeResult,
    GateResolveFinding, GateRun, GateRunInfo, GateRunStarted, GateShip, GateShipResult, GateStatus,
    GateStatusResult, ProtocolError,
};
use ulid::Ulid;

use crate::api::{inject_agent_env, needs_input_score, store_err};
use crate::server::AppState;
use crate::tokens::TokenScope;

/// Needs You kind for an escalated finding (`data-model.md` / needs_you_items.kind).
const NEEDS_YOU_GATE_FINDING: &str = "gate_finding";

/// How long to wait for a gate reviewer/fixer session to run to completion.
fn session_timeout() -> Duration {
    let ms = std::env::var("DFLOW_GATE_SESSION_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60_000);
    Duration::from_millis(ms)
}

// ---- owner verbs ----

/// `gate.run { card_id, head_sha?, author_harness?, reviewer_harness? }`: resolve the
/// card/project/recipe, open a gate run, and kick off the pipeline on a background thread.
pub fn gate_run(state: &AppState, req: GateRun) -> Result<GateRunStarted, ProtocolError> {
    let card = state
        .store
        .get_card(&req.card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("card {}", req.card_id)))?;
    let project_id = card
        .project_id
        .clone()
        .ok_or_else(|| ProtocolError::bad_request("card has no project; the gate needs a project"))?;
    let project = state
        .store
        .get_project(&project_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("project {project_id}")))?;

    // Resolve the recipe to read its verify block (gate strictness + reviewer harness).
    let resolved = crate::recipes::resolve_dispatch_recipe(state, None, &card, &project)?;
    let recipe = Arc::new(resolved.recipe.clone());
    let strictness = recipe.gate().unwrap_or(GateStrictness::Full);

    // The commit under test: an explicit head_sha, else the card's authoring worktree HEAD.
    let head_sha = match req.head_sha.clone().filter(|s| !s.trim().is_empty()) {
        Some(s) => s,
        None => authoring_head(state, &project_id, &req.card_id)
            .ok_or_else(|| ProtocolError::bad_request(
                "no commit to gate: pass head_sha, or dispatch the card so its worktree has a HEAD",
            ))?,
    };

    // The implementing harness (for the reviewer-differs check): explicit, else the card's
    // most recent session harness, else "unknown".
    let author_harness = req
        .author_harness
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| latest_session_harness(state, &req.card_id))
        .unwrap_or_else(|| "unknown".to_string());

    // The reviewer harness: an explicit override, else the recipe's setting resolved.
    let reviewer_setting = req
        .reviewer_harness
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| recipe.verify.as_ref().map(|v| v.reviewer_harness.clone()))
        .unwrap_or_else(|| "different".to_string());
    let reviewer_harness = resolve_reviewer_harness(&reviewer_setting, &author_harness, state)
        .map_err(ProtocolError::bad_request)?;

    let run = state
        .store
        .create_gate_run(NewGateRun {
            card_id: req.card_id.clone(),
            worktree_id: None,
            gate_strictness: Some(strictness_str(strictness).to_string()),
            author_harness: Some(author_harness.clone()),
            reviewer_harness: Some(reviewer_harness.clone()),
            head_sha: Some(head_sha.clone()),
            branch: Some(gate_branch(&req.card_id)),
            output_path: None,
        })
        .map_err(store_err)?;

    let ctx = PipelineCtx {
        gate_run_id: run.id.clone(),
        card_id: req.card_id.clone(),
        project,
        recipe,
        strictness,
        author_harness,
        reviewer_harness,
        head_sha,
    };
    let state_clone = state.clone();
    let run_id = run.id.clone();
    if let Err(e) = std::thread::Builder::new()
        .name(format!("gate-{}", &run.id))
        .spawn(move || run_pipeline(state_clone, ctx))
    {
        tracing::error!(%e, "could not spawn gate pipeline thread");
        let _ = state.store.finish_gate_run(&run_id, gate_status::FAILED, Some("could not start pipeline"));
    }

    Ok(GateRunStarted {
        gate_run_id: run.id,
        status: gate_status::RUNNING.to_string(),
        strictness: strictness_str(strictness).to_string(),
    })
}

/// `gate.status { card_id }`: the latest gate run for a card plus its findings.
pub fn gate_status(state: &AppState, req: GateStatus) -> Result<GateStatusResult, ProtocolError> {
    let run = state.store.latest_gate_run_for_card(&req.card_id).map_err(store_err)?;
    let findings = match &run {
        Some(r) => state.store.findings_for_run(&r.id).map_err(store_err)?,
        None => Vec::new(),
    };
    Ok(GateStatusResult {
        run: run.map(gate_run_info),
        findings: findings.into_iter().map(finding_info).collect(),
    })
}

/// `gate.resolve_finding { finding_id, resolution }`: the human's escalation decision in
/// Plan Studio chrome (`gate.md` / Escalation: approve / fix / skip). Resolving the last
/// open finding on the run also resolves the `gate_finding` Needs You item.
pub fn gate_resolve_finding(
    state: &AppState,
    req: GateResolveFinding,
) -> Result<FindingResult, ProtocolError> {
    let res = normalize_resolution(&req.resolution)?;
    let finding = state
        .store
        .get_finding(&req.finding_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("finding {}", req.finding_id)))?;
    let updated = state
        .store
        .resolve_finding(&req.finding_id, res)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("finding {}", req.finding_id)))?;

    // If no findings remain open on the run, clear the escalation Needs You.
    let open = state
        .store
        .findings_for_run(&finding.gate_run_id)
        .map_err(store_err)?
        .into_iter()
        .filter(|f| f.is_open())
        .count();
    if open == 0 {
        let _ = state.store.resolve_needs_you(
            &finding.card_id,
            &format!("{NEEDS_YOU_GATE_FINDING}:{}", finding.gate_run_id),
            "ui",
        );
    }
    Ok(FindingResult { finding: finding_info(updated) })
}

// ---- ship path (`gate.md` / Ship, Teardown safety; `roadmap.md` M5.4) ----

/// `gate.ship { card_id }`: only a passed gate ships. In `pr` mode the passed branch is
/// pushed through the git credential helper and a PR opened via gh with a generated
/// summary (including `Fixes #<n>` for a GitHub-issue origin card); in `local_only`/
/// `local_merge` mode the default branch fast-forwards to the gate head instead.
pub fn gate_ship(state: &AppState, req: GateShip) -> Result<GateShipResult, ProtocolError> {
    let (card, project, run) = ship_context(state, &req.card_id)?;
    if run.status != gate_status::PASSED {
        return Err(ProtocolError::bad_request(format!(
            "the gate has not passed (status: {}); nothing ships until every check is green and every finding resolved",
            run.status
        )));
    }
    let head_sha = run.head_sha.clone().ok_or_else(|| ProtocolError::internal("gate run has no head_sha"))?;
    let branch = run.branch.clone().unwrap_or_else(|| gate_branch(&req.card_id));
    let cwd = gate_worktree_path(state, &run)?;

    // Effective ship mode: local when the project is local_only or the recipe ships by
    // local merge; none when the recipe ships nothing; else a pull request.
    let recipe = crate::recipes::resolve_dispatch_recipe(state, None, &card, &project)?.recipe;
    let ship_target = recipe.ship.as_ref().map(|s| s.target).unwrap_or(ShipTarget::Pr);
    if ship_target == ShipTarget::None {
        return Ok(GateShipResult { mode: "none".into(), pushed: false, pr_number: None, pr_url: None, message: "the recipe ships nothing (ship.target: none)".into() });
    }
    let local = project.mode == "local_only" || ship_target == ShipTarget::LocalMerge;

    if local {
        return ship_local_merge(state, &card, &project, &run, &head_sha);
    }

    // PR mode: gh must be usable, else degrade to a local-only signal (`gate.md`).
    let auth = Gh::from_env().auth_status();
    if !auth.usable() {
        return Ok(GateShipResult {
            mode: "pr".into(),
            pushed: false,
            pr_number: None,
            pr_url: None,
            message: "gh is not authenticated, so PR mode is unavailable; run `gh auth login`, or set this project to local_only to ship by local fast-forward merge".into(),
        });
    }

    // Push the gate branch through the system git CLI (the user's credential helper), the
    // same path their manual pushes use (`gate.md`: push stays git-CLI, never gh).
    push_branch(&cwd, "origin", &branch)
        .map_err(|e| ProtocolError::internal(format!("git push failed: {e}")))?;
    let _ = state.store.append_card_event(
        &req.card_id,
        dflow_core::event_kind::PUSHED,
        serde_json::json!({ "branch": branch, "head_sha": head_sha, "remote": "origin" }),
    );

    // Open the PR via gh with the generated summary + Fixes line + card link.
    let fixes = issue_number_for(&card);
    let body = compose_pr_body(&card, fixes);
    let title = pr_title(&card);
    let pr = Gh::from_env()
        .pr_create(&cwd, &PrCreate { title, body, base: project.default_branch.clone(), head: branch.clone() })
        .map_err(crate::github::gh_err)?;
    let _ = state.store.set_gate_run_pr(&run.id, pr.number as i64, &pr.url);
    let _ = state.store.append_card_event(
        &req.card_id,
        dflow_core::event_kind::PR_OPENED,
        serde_json::json!({ "pr_number": pr.number, "pr_url": pr.url, "base": project.default_branch, "head": branch, "fixes": fixes }),
    );
    let score = needs_input_score(state, Some(&req.card_id));
    let _ = state.store.raise_needs_you(&req.card_id, "pr_ready", &format!("pr_ready:{}", pr.number), score.max(50));

    Ok(GateShipResult {
        mode: "pr".into(),
        pushed: true,
        pr_number: Some(pr.number as i64),
        pr_url: Some(pr.url.clone()),
        message: format!("pushed {branch} and opened PR #{}", pr.number),
    })
}

/// `gate.merge { card_id, method? }`: watch CI via `gh pr checks`, merge (squash default),
/// then prove the work landed before returning the worktree (`gate.md` / Ship, Teardown
/// safety: a worktree returns only when its work is provably landed).
pub fn gate_merge(state: &AppState, req: GateMerge) -> Result<GateMergeResult, ProtocolError> {
    let (card, project, run) = ship_context(state, &req.card_id)?;
    let pr_number = run.pr_number.ok_or_else(|| {
        ProtocolError::bad_request("no PR to merge for this card; run gate.ship first")
    })? as u64;
    let cwd = gate_worktree_path(state, &run)?;
    let gh = Gh::from_env();

    // Watch CI: bounded poll until checks settle. An empty check set (no CI) is fine.
    let ci = watch_ci(state, &gh, &cwd, &req.card_id, pr_number);
    if ci.failing {
        let score = needs_input_score(state, Some(&req.card_id));
        let _ = state.store.raise_needs_you(&req.card_id, "gate_finding", &format!("ci_failed:{pr_number}"), score.max(70));
        return Ok(GateMergeResult { merged: false, pr_number: Some(pr_number as i64), landed: false, message: format!("CI is failing on PR #{pr_number}; merge refused until it is green") });
    }

    // Merge (squash default).
    let method = match req.method.as_deref() {
        Some("merge") => MergeMethod::Merge,
        Some("rebase") => MergeMethod::Rebase,
        _ => MergeMethod::Squash,
    };
    gh.pr_merge(&cwd, pr_number, method, false).map_err(crate::github::gh_err)?;
    let _ = state.store.append_card_event(
        &req.card_id,
        dflow_core::event_kind::MERGED,
        serde_json::json!({ "pr_number": pr_number, "method": merge_method_str(method) }),
    );

    // Teardown landed-work proof: PR merged (gh state), or head reachable from the
    // default branch. Only then does the gate worktree return (`gate.md`).
    let landed = prove_landed(&gh, &cwd, pr_number, run.head_sha.as_deref(), &project.default_branch);
    if landed {
        finalize_ship_worktree(state, &card, &project, &run, &cwd, true);
        Ok(GateMergeResult { merged: true, pr_number: Some(pr_number as i64), landed: true, message: format!("merged PR #{pr_number} and returned the worktree (work provably landed)") })
    } else {
        // Refuse to reset an unlanded worktree: park it dirty with a Needs You.
        finalize_ship_worktree(state, &card, &project, &run, &cwd, false);
        Ok(GateMergeResult { merged: true, pr_number: Some(pr_number as i64), landed: false, message: format!("PR #{pr_number} merge reported but landing is not provable; the worktree is parked (dirty) and nothing was reset") })
    }
}

/// Ship a passed gate by fast-forwarding the project's default branch to the gate head
/// (`gate.md` / Project modes: local_only ends with an approved local fast-forward merge).
fn ship_local_merge(
    state: &AppState,
    card: &dflow_proto::Card,
    project: &dflow_proto::Project,
    run: &GateRunRow,
    head_sha: &str,
) -> Result<GateShipResult, ProtocolError> {
    // Fast-forward the checked-out default branch in the project's main checkout.
    match git_capture(Path::new(&project.path), &["merge", "--ff-only", head_sha]) {
        Ok(_) => {
            let _ = state.store.append_card_event(
                &card.id,
                dflow_core::event_kind::MERGED,
                serde_json::json!({ "mode": "local_merge", "head_sha": head_sha, "branch": project.default_branch }),
            );
            let cwd = gate_worktree_path(state, run).ok();
            if let Some(cwd) = cwd {
                finalize_ship_worktree(state, card, project, run, &cwd, true);
            }
            Ok(GateShipResult { mode: "local_merge".into(), pushed: false, pr_number: None, pr_url: None, message: format!("fast-forwarded {} to the gate head", project.default_branch) })
        }
        Err(e) => Err(ProtocolError::bad_request(format!(
            "cannot fast-forward {} to the gate head ({e}); the default branch advanced - rebase the work first",
            project.default_branch
        ))),
    }
}

// ---- agent verb ----

/// `finding.add` (agent scope): a reviewer session files a structured finding against its
/// active gate run (`gate.md` / Adversarial review). The token carries the gate run id.
pub fn finding_add(
    state: &AppState,
    token: &crate::tokens::AgentToken,
    req: FindingAdd,
) -> Result<FindingAddResult, ProtocolError> {
    let gate_run_id = token.gate_run_id.clone().ok_or_else(|| {
        ProtocolError::forbidden("finding.add is only available to a gate reviewer session")
    })?;
    let card_id = token
        .card_id
        .clone()
        .ok_or_else(|| ProtocolError::forbidden("gate token has no card"))?;
    let body = req.body.trim();
    if body.is_empty() {
        return Err(ProtocolError::bad_request(
            "a finding needs a concrete failure scenario or rule citation, not an empty body",
        ));
    }
    if !severity::is_valid(&req.severity) {
        return Err(ProtocolError::bad_request(format!(
            "severity must be blocker|major|minor, got '{}'",
            req.severity
        )));
    }
    let cat = req.category.as_deref().unwrap_or(category::INTENT);
    if !category::is_valid(cat) {
        return Err(ProtocolError::bad_request(format!(
            "category must be mechanical|intent, got '{cat}'"
        )));
    }
    let finding = state
        .store
        .add_finding(&gate_run_id, &card_id, &req.severity, cat, "reviewer", body, req.evidence.as_deref())
        .map_err(store_err)?;
    Ok(FindingAddResult {
        finding_id: finding.id,
        gate_run_id,
        severity: finding.severity,
        category: finding.category,
    })
}

// ---- pipeline ----

/// The data the background pipeline needs, owned so the thread can outlive the request.
struct PipelineCtx {
    gate_run_id: String,
    card_id: String,
    project: dflow_proto::Project,
    recipe: Arc<Recipe>,
    strictness: GateStrictness,
    author_harness: String,
    reviewer_harness: String,
    head_sha: String,
}

/// Run the whole gate pipeline for one run (`gate.md` / Pipeline). Any hard error
/// finishes the run failed with a reason on the timeline.
fn run_pipeline(state: AppState, ctx: PipelineCtx) {
    if let Err(reason) = run_pipeline_inner(&state, &ctx) {
        tracing::warn!(gate_run = %ctx.gate_run_id, %reason, "gate pipeline failed");
        let _ = state.store.finish_gate_run(&ctx.gate_run_id, gate_status::FAILED, Some(&reason));
        raise_gate_needs_you(&state, &ctx);
    }
}

fn run_pipeline_inner(state: &AppState, ctx: &PipelineCtx) -> Result<(), String> {
    // gate: none -> nothing to gate; pass immediately (a shipless/none recipe).
    if ctx.strictness == GateStrictness::None {
        state
            .store
            .record_gate_step(&ctx.card_id, &ctx.gate_run_id, gate_step::DONE, "passed", serde_json::json!({ "note": "gate: none" }))
            .map_err(|e| e.to_string())?;
        state.store.finish_gate_run(&ctx.gate_run_id, gate_status::PASSED, None).map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Lease a gate-class worktree (a different slot than the still-leased author one).
    let wt = state.worktrees.lease(&ctx.project, &ctx.card_id).map_err(|e| e.to_string())?;
    let cwd = PathBuf::from(&wt.path);
    let _ = state.store.set_gate_run_worktree(&ctx.gate_run_id, &wt.id);
    let evidence_dir = state.data_dir.root().join("gate").join(&ctx.gate_run_id);
    let _ = std::fs::create_dir_all(&evidence_dir);
    let _ = state.store.set_gate_run_output_path(&ctx.gate_run_id, &evidence_dir.to_string_lossy());

    // Check out the commit under test with warm caches intact.
    checkout_head(&cwd, &ctx.head_sha).map_err(|e| format!("checkout {}: {e}", ctx.head_sha))?;

    // Materialize the env in checks-only mode (`gate.md`: env materialized in checks-only
    // mode). Register its secrets with the scrubber so gate evidence/events redact them.
    let materialized = state
        .env_vault
        .materialize(&state.store, &ctx.project.id, &cwd)
        .map_err(|e| e.to_string())?;
    let secret_key = format!("gate:{}", ctx.gate_run_id);
    if !materialized.secret_values.is_empty() {
        dflow_core::secret::registry().register(&secret_key, materialized.secret_values.clone());
    }
    let base_env = materialized.env.clone();

    // 1) Checks.
    let checks_ok = run_checks(state, ctx, &cwd, &base_env, &materialized.secret_values, &evidence_dir)?;
    if !checks_ok {
        state.store.finish_gate_run(&ctx.gate_run_id, gate_status::FAILED, Some("checks failed")).map_err(|e| e.to_string())?;
        raise_gate_needs_you(state, ctx);
        finalize_worktree(state, ctx, &wt.id, &cwd, &secret_key);
        return Ok(());
    }

    // checks_only: green checks are the whole gate.
    if ctx.strictness == GateStrictness::ChecksOnly {
        state.store.finish_gate_run(&ctx.gate_run_id, gate_status::PASSED, None).map_err(|e| e.to_string())?;
        // Keep the worktree leased for a ship step; shred is deferred to ship teardown.
        return Ok(());
    }

    // 2) Adversarial review (full gate). Reviewer must differ from the author.
    if ctx.reviewer_harness == ctx.author_harness {
        let reason = format!(
            "reviewer harness '{}' must differ from the author's; adversarial review needs a different harness",
            ctx.reviewer_harness
        );
        state
            .store
            .record_gate_step(&ctx.card_id, &ctx.gate_run_id, gate_step::REVIEW, "failed", serde_json::json!({ "reason": reason }))
            .map_err(|e| e.to_string())?;
        state.store.finish_gate_run(&ctx.gate_run_id, gate_status::FAILED, Some(&reason)).map_err(|e| e.to_string())?;
        raise_gate_needs_you(state, ctx);
        finalize_worktree(state, ctx, &wt.id, &cwd, &secret_key);
        return Ok(());
    }
    run_review(state, ctx, &cwd, &base_env, &wt.id)?;

    // 3) Autofix safe-mechanical findings, then re-check.
    run_autofix(state, ctx, &cwd, &base_env, &wt.id, &materialized.secret_values, &evidence_dir)?;

    // 4) Escalate intent-touching findings; else pass.
    let open: Vec<FindingRow> = state
        .store
        .findings_for_run(&ctx.gate_run_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|f| f.is_open())
        .collect();
    if open.is_empty() {
        state.store.finish_gate_run(&ctx.gate_run_id, gate_status::PASSED, None).map_err(|e| e.to_string())?;
        // Passed full gate: keep the worktree leased for ship.
        return Ok(());
    }

    // Escalation: intent findings become a Needs You item for the human to resolve.
    state
        .store
        .record_gate_step(
            &ctx.card_id,
            &ctx.gate_run_id,
            gate_step::ESCALATE,
            "escalated",
            serde_json::json!({ "open_findings": open.len() }),
        )
        .map_err(|e| e.to_string())?;
    state.store.finish_gate_run(&ctx.gate_run_id, gate_status::ESCALATED, Some("intent findings need you")).map_err(|e| e.to_string())?;
    raise_gate_needs_you(state, ctx);
    finalize_worktree(state, ctx, &wt.id, &cwd, &secret_key);
    Ok(())
}

/// Run every registered check command in order, capturing scrubbed evidence. A failed
/// check becomes a blocker finding (source=check) and returns `false`.
fn run_checks(
    state: &AppState,
    ctx: &PipelineCtx,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    secrets: &[String],
    evidence_dir: &Path,
) -> Result<bool, String> {
    state.store.set_gate_run_step(&ctx.gate_run_id, gate_step::CHECKS).map_err(|e| e.to_string())?;
    let checks = &ctx.project.check_cmds;
    if checks.is_empty() {
        state
            .store
            .record_gate_step(&ctx.card_id, &ctx.gate_run_id, gate_step::CHECKS, "passed", serde_json::json!({ "note": "no check commands configured" }))
            .map_err(|e| e.to_string())?;
        return Ok(true);
    }
    let mut all_ok = true;
    for check in checks {
        let (success, code, output) = run_shell(cwd, &check.cmd, env);
        let scrubbed = scrub(&output, secrets);
        let log_path = evidence_dir.join(format!("check-{}.log", sanitize(&check.name)));
        let _ = std::fs::write(&log_path, &scrubbed);
        state
            .store
            .record_gate_step(
                &ctx.card_id,
                &ctx.gate_run_id,
                gate_step::CHECKS,
                if success { "passed" } else { "failed" },
                serde_json::json!({
                    "check": check.name,
                    "cmd": check.cmd,
                    "exit_code": code,
                    "log": log_path.to_string_lossy(),
                }),
            )
            .map_err(|e| e.to_string())?;
        if !success {
            all_ok = false;
            let _ = state.store.add_finding(
                &ctx.gate_run_id,
                &ctx.card_id,
                severity::BLOCKER,
                category::INTENT,
                "check",
                &format!("check '{}' failed (exit {}): {}", check.name, code, check.cmd),
                Some(&log_path.to_string_lossy()),
            );
        }
    }
    Ok(all_ok)
}

/// Spawn a reviewer session on the reviewer harness with the diff + acceptance + plan,
/// wait for it to finish, and record the review step. Findings arrive via `finding.add`.
fn run_review(
    state: &AppState,
    ctx: &PipelineCtx,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    worktree_id: &str,
) -> Result<(), String> {
    state.store.set_gate_run_step(&ctx.gate_run_id, gate_step::REVIEW).map_err(|e| e.to_string())?;
    let diff = git_capture(cwd, &["diff", &format!("{}...HEAD", ctx.project.default_branch)]).unwrap_or_default();
    let brief = compose_reviewer_brief(ctx, &diff);
    let sid = spawn_gate_session(state, &ctx.reviewer_harness, &brief, cwd, ctx, env, worktree_id)?;
    let finished = wait_for_session_exit(state, &sid, session_timeout());
    let findings = state.store.findings_for_run(&ctx.gate_run_id).map_err(|e| e.to_string())?;
    state
        .store
        .record_gate_step(
            &ctx.card_id,
            &ctx.gate_run_id,
            gate_step::REVIEW,
            if finished { "passed" } else { "timeout" },
            serde_json::json!({
                "reviewer_harness": ctx.reviewer_harness,
                "session_id": sid,
                "findings": findings.len(),
                "session_finished": finished,
            }),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Autofix: hand every open safe-mechanical finding to a fixer session, then mark those
/// findings `autofixed` only when the autofix EARNS the claim (`gate.md` / Autofix). A
/// mechanical finding resolves `autofixed` only when BOTH observable conditions hold: the
/// fixer actually CHANGED the worktree - a new commit, or an uncommitted working-tree diff
/// the gate then commits attributable to the fixer - AND the re-check stays GREEN. Anything
/// else escalates honestly: the mechanical findings stay open (so the pipeline routes them
/// to a `gate_finding` Needs You) and the reason is recorded in the autofix `gate_step`
/// evidence with the fixer's tail output. This closes the audit's false pass: the defect
/// class autofix targets (lint, dead imports, formatting) does not fail checks, so a green
/// re-check alone never proves a fix landed. (Session completion is not a usable gate here:
/// on Windows ConPTY a cleanly-exited fixer is never observed to exit, so `finished` only
/// refines the escalation reason - it never blocks a real, green change from autofixing.)
fn run_autofix(
    state: &AppState,
    ctx: &PipelineCtx,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    worktree_id: &str,
    secrets: &[String],
    evidence_dir: &Path,
) -> Result<(), String> {
    let mechanical: Vec<FindingRow> = state
        .store
        .findings_for_run(&ctx.gate_run_id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|f| f.is_open() && f.category == category::MECHANICAL)
        .collect();
    if mechanical.is_empty() {
        return Ok(());
    }
    state.store.set_gate_run_step(&ctx.gate_run_id, gate_step::AUTOFIX).map_err(|e| e.to_string())?;
    let fixer_harness = fixer_harness(&ctx.author_harness);
    let brief = compose_fixer_brief(ctx, &mechanical);

    // Capture the pre-fixer tree state so any change can be attributed to the fixer.
    let before_head = git_head_sha(cwd);
    let before_status = git_status_porcelain(cwd);

    let sid = spawn_gate_session(state, &fixer_harness, &brief, cwd, ctx, env, worktree_id)?;
    let finished = wait_for_session_exit(state, &sid, session_timeout());

    // Grab the fixer's tail output for evidence before the session is reaped; scrubbed.
    let tail = fixer_tail(state, &sid, secrets);
    let tail_log = evidence_dir.join("autofix-fixer.log");
    let _ = std::fs::write(&tail_log, &tail);
    let tail_log = tail_log.to_string_lossy().into_owned();

    // Earned-claim: mark `autofixed` only when the fixer actually CHANGED the worktree and
    // the re-check stays GREEN. `finished` refines the escalation reason but must NOT gate
    // success: on Windows ConPTY the daemon holds the PTY master for the session's whole
    // life, so a cleanly-exited fixer is never observed to exit (`is_alive` stays true until
    // we kill it at the session timeout). Gating success on `finished` would falsely escalate
    // every autofix. The observable proxy for "the fixer did the work" is a real change plus a
    // green re-check, which also subsumes the "killed before it did anything" case (no change
    // -> escalate).
    let Some(change) = attribute_fixer_change(cwd, &before_head, &before_status)? else {
        // No change landed. A fixer we had to kill at the timeout "did not complete"; one we
        // observed exit cleanly "made no changes".
        return escalate_autofix(state, ctx, &fixer_harness, &sid, no_change_reason(finished), &tail, &tail_log);
    };
    // The fixer's change advanced HEAD; record it for the ship path and the evidence.
    let _ = state.store.set_gate_run_head(&ctx.gate_run_id, &change.head);

    // (c) The re-check must stay green after the fixer's change.
    let rechecked = run_checks(state, ctx, cwd, env, secrets, evidence_dir)?;
    if !rechecked {
        // The fixer changed code but the checks now fail: do not claim autofixed. The
        // still-open mechanical findings (plus any new check finding) escalate.
        state
            .store
            .record_gate_step(
                &ctx.card_id,
                &ctx.gate_run_id,
                gate_step::AUTOFIX,
                "failed",
                serde_json::json!({
                    "fixer_harness": fixer_harness,
                    "session_id": sid,
                    "session_finished": finished,
                    "reason": "re-check failed after autofix",
                    "fixer_commit": change.head,
                    "diffstat": change.diffstat,
                    "gate_committed_worktree": change.gate_committed,
                    "fixer_tail_log": tail_log,
                }),
            )
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Earned: the fixer completed, changed the worktree, and the re-check is green.
    for f in &mechanical {
        let _ = state.store.resolve_finding(&f.id, resolution::AUTOFIXED);
    }
    state
        .store
        .record_gate_step(
            &ctx.card_id,
            &ctx.gate_run_id,
            gate_step::AUTOFIX,
            "passed",
            serde_json::json!({
                "fixer_harness": fixer_harness,
                "session_id": sid,
                "session_finished": finished,
                "applied": mechanical.len(),
                "rechecked_green": rechecked,
                "fixer_commit": change.head,
                "diffstat": change.diffstat,
                "gate_committed_worktree": change.gate_committed,
            }),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A change the fixer landed in the gate worktree, attributed for the earned autofix claim.
struct FixerChange {
    /// The commit sha now at HEAD - the fixer's own commit, or the gate's commit of a
    /// working-tree diff the fixer left uncommitted.
    head: String,
    /// A short diffstat from the pre-fixer HEAD to `head`, for evidence.
    diffstat: String,
    /// Whether the gate itself committed a working-tree diff the fixer left uncommitted.
    gate_committed: bool,
}

/// Attribute a fixer's change to the gate worktree (`gate.md` / Autofix, earned-claim
/// criterion (b)). A new commit is the fixer's own; an uncommitted working-tree diff is
/// committed by the gate and attributed to the fixer. Returns `None` when the fixer changed
/// nothing (HEAD unmoved and the porcelain status identical to before it ran - which
/// ignores pre-existing dirt like a reviewer session's marker file).
fn attribute_fixer_change(
    cwd: &Path,
    before_head: &str,
    before_status: &str,
) -> Result<Option<FixerChange>, String> {
    let after_head = git_head_sha(cwd);
    if !after_head.is_empty() && after_head != before_head {
        // The fixer committed its own change; that commit is the attribution.
        let diffstat = git_capture(cwd, &["diff", "--stat", before_head, &after_head]).unwrap_or_default();
        return Ok(Some(FixerChange { head: after_head, diffstat, gate_committed: false }));
    }

    // No new commit: did the fixer leave an uncommitted working-tree diff?
    if git_status_porcelain(cwd).trim() == before_status.trim() {
        return Ok(None); // nothing changed since before the fixer ran
    }
    // Stage and commit the fixer's working-tree diff, attributable to the fixer. If nothing
    // is actually staged (e.g. only ignored churn), there is no real change.
    git_capture(cwd, &["add", "-A"])?;
    if git_capture(cwd, &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(None);
    }
    git_capture(cwd, &["commit", "-m", "gate(autofix): commit fixer working-tree changes"])?;
    let head = git_head_sha(cwd);
    if head.is_empty() || head == before_head {
        return Ok(None);
    }
    let diffstat = git_capture(cwd, &["diff", "--stat", before_head, &head]).unwrap_or_default();
    Ok(Some(FixerChange { head, diffstat, gate_committed: true }))
}

/// `git rev-parse HEAD`, trimmed, or empty on error.
fn git_head_sha(cwd: &Path) -> String {
    git_capture(cwd, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// `git status --porcelain`, or empty on error (a stable fingerprint of the working tree).
fn git_status_porcelain(cwd: &Path) -> String {
    git_capture(cwd, &["status", "--porcelain"]).unwrap_or_default()
}

/// The fixer session's tail output (last lines of the visible screen), scrubbed of secrets,
/// for autofix escalation evidence. Empty when the session is already gone.
fn fixer_tail(state: &AppState, session_id: &str, secrets: &[String]) -> String {
    state
        .sessions
        .get_str(session_id)
        .map(|s| s.peek_scrubbed(40, secrets))
        .unwrap_or_default()
}

/// Record an escalated autofix step and leave the mechanical findings open, so the pipeline
/// escalates them to a `gate_finding` Needs You (`gate.md` / Autofix earned-claim,
/// Escalation). The `reason` and the fixer's tail output are the honest evidence.
fn escalate_autofix(
    state: &AppState,
    ctx: &PipelineCtx,
    fixer_harness: &str,
    session_id: &str,
    reason: &str,
    tail: &str,
    tail_log: &str,
) -> Result<(), String> {
    state
        .store
        .record_gate_step(
            &ctx.card_id,
            &ctx.gate_run_id,
            gate_step::AUTOFIX,
            "escalated",
            serde_json::json!({
                "fixer_harness": fixer_harness,
                "session_id": session_id,
                "reason": reason,
                "fixer_tail": preview_tail(tail),
                "fixer_tail_log": tail_log,
            }),
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A short inline preview of the fixer tail for the event payload (the full text is in the
/// pointed-at log), truncated on a char boundary.
fn preview_tail(tail: &str) -> String {
    const MAX: usize = 600;
    let t = tail.trim();
    if t.len() <= MAX {
        return t.to_string();
    }
    let mut cut = MAX;
    while !t.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", &t[..cut])
}

/// The honest escalation reason when the fixer changed nothing (`gate.md` / Autofix earned
/// claim). A fixer we had to kill at the session timeout "did not complete"; one we observed
/// exit on its own without changing anything "made no changes".
fn no_change_reason(finished: bool) -> &'static str {
    if finished {
        "autofix made no changes"
    } else {
        "fixer did not complete"
    }
}

// ---- session spawning ----

/// Spawn a gate reviewer/fixer session: mint a per-task token carrying the gate run id,
/// inject the `dflow` CLI env, launch the harness in the gate worktree, and return the
/// session id. Reuses the `DFLOW_LAUNCH_<HARNESS>` seam, so tests substitute a stub.
fn spawn_gate_session(
    state: &AppState,
    harness_name: &str,
    brief: &str,
    cwd: &Path,
    ctx: &PipelineCtx,
    base_env: &BTreeMap<String, String>,
    worktree_id: &str,
) -> Result<String, String> {
    // NOTE: the gate fixer/reviewer brief still rides in as a launch argument. On a shim
    // harness this has the same `cmd.exe` multi-line truncation exposure as the dispatch
    // brief did; the gate harness is claude by default (native exe, unaffected). Routing the
    // gate brief through the typed-delivery path (`harness::brief_delivery`) is a tracked
    // sibling follow-up, out of scope for the dispatch-brief-delivery fix.
    let command = harness::harness_command(harness_name, Some(brief), None, None)
        .ok_or_else(|| format!("harness '{harness_name}' is not launchable (no manifest or DFLOW_LAUNCH override)"))?;

    let (task_token, token_handle) = state.tokens.mint(TokenScope {
        card_id: Some(ctx.card_id.clone()),
        project_id: Some(ctx.project.id.clone()),
        audit: false,
        budget_cards: None,
        budget_notes: None,
        recipe: None,
        gate_run_id: Some(ctx.gate_run_id.clone()),
    });
    let mut env = base_env.clone();
    inject_agent_env(state, &mut env, &task_token, Some(&ctx.card_id));

    let card_ulid = Ulid::from_string(&ctx.card_id).ok();
    let worktree_ulid = Ulid::from_string(worktree_id).ok();
    let spec = SessionSpec {
        harness: harness_name.to_string(),
        command,
        cols: 120,
        rows: 32,
        cwd: Some(cwd.to_path_buf()),
        env,
        card_id: card_ulid,
        project_id: Some(ctx.project.id.clone()),
        worktree_id: worktree_ulid,
        first_prompt: Some(harness::preview(brief, 120)),
        scrollback_dir: Some(state.data_dir.scrollback_dir()),
        ..Default::default()
    };
    let session = state.sessions.create(spec).map_err(|e| format!("could not launch gate harness '{harness_name}': {e}"))?;
    token_handle.bind_session(&session.id.to_string());
    Ok(session.id.to_string())
}

/// Poll a session until its PTY exits, bounded by `timeout`. Returns whether it finished.
fn wait_for_session_exit(state: &AppState, session_id: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match state.sessions.get_str(session_id) {
            Some(s) if s.is_alive() => {}
            _ => {
                // Give the daemon a beat to drain the token/finalize, then report done.
                std::thread::sleep(Duration::from_millis(200));
                return true;
            }
        }
        if Instant::now() >= deadline {
            let _ = state.sessions.get_str(session_id).map(|s| s.kill());
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

// ---- teardown ----

/// Reset a gate-class worktree back to the pool. A gate worktree is a disposable checkout
/// (the author's commit lives safely in the object store and the author worktree), so it
/// is force-reset to the default branch (bypassing the authoring-work unlanded check,
/// which does not apply here), warm caches preserved. Secrets are shredded first.
fn finalize_worktree(state: &AppState, ctx: &PipelineCtx, worktree_id: &str, cwd: &Path, secret_key: &str) {
    dflow_core::secret::registry().unregister(secret_key);
    match state.env_vault.cleanup(&state.store, &ctx.project.id, cwd) {
        Ok(n) if n > 0 => tracing::info!(gate_run = %ctx.gate_run_id, shredded = n, "shredded gate secrets"),
        _ => {}
    }
    let _ = git_capture(cwd, &["checkout", "--force", "--detach", &ctx.project.default_branch]);
    let _ = git_capture(cwd, &["reset", "--hard"]);
    let _ = git_capture(cwd, &["clean", "-fd"]);
    if let Err(e) = state.store.set_worktree_lease(worktree_id, dflow_core::lease_state::AVAILABLE, None) {
        tracing::warn!(%e, "could not mark gate worktree available");
    }
    let _ = state.store.append_card_event(
        &ctx.card_id,
        dflow_core::event_kind::WORKTREE_RETURNED,
        serde_json::json!({ "worktree_id": worktree_id, "outcome": "gate_reset" }),
    );
}

// ---- helpers ----

/// Resolve the reviewer harness (`gate.md`: a different harness than the author). A
/// concrete adapter name is used as-is; `different` picks the first available family that
/// is not the author's, erroring when none exists.
pub fn resolve_reviewer_harness(
    setting: &str,
    author: &str,
    state: &AppState,
) -> Result<String, String> {
    if setting != "different" {
        return Ok(setting.to_string());
    }
    let mut candidates = available_harnesses(state);
    candidates.retain(|h| h != author);
    candidates.into_iter().next().ok_or_else(|| {
        format!(
            "recipe wants a reviewer on a different harness than the author ('{author}'), but no \
             other harness is available; add another launcher or set a concrete reviewer_harness"
        )
    })
}

/// The harness names available as reviewers: enabled launcher adapters plus the built-in
/// dispatchable families, deduped.
fn available_harnesses(state: &AppState) -> Vec<String> {
    let mut out: Vec<String> = vec!["claude".into(), "codex".into(), "opencode".into()];
    if let Ok(agents) = state.store.list_agents() {
        for a in agents.into_iter().filter(|a| a.enabled) {
            if !out.contains(&a.adapter) {
                out.push(a.adapter);
            }
        }
    }
    out
}

/// The fixer harness: the author's own harness reworks its own code, unless a test seam
/// overrides it (`DFLOW_GATE_FIXER_HARNESS`).
fn fixer_harness(author: &str) -> String {
    std::env::var("DFLOW_GATE_FIXER_HARNESS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| author.to_string())
}

/// The HEAD of the card's authoring worktree (the commit under test).
fn authoring_head(state: &AppState, project_id: &str, card_id: &str) -> Option<String> {
    let worktrees = state.store.worktrees_for_project(project_id).ok()?;
    let wt = worktrees
        .into_iter()
        .filter(|w| w.leased_by_card.as_deref() == Some(card_id))
        .max_by(|a, b| a.updated_at.cmp(&b.updated_at))?;
    let head = git_capture(Path::new(&wt.path), &["rev-parse", "HEAD"]).ok()?;
    let head = head.trim().to_string();
    if head.is_empty() {
        None
    } else {
        Some(head)
    }
}

/// The most recent session's harness for a card (the implementing harness).
fn latest_session_harness(state: &AppState, card_id: &str) -> Option<String> {
    let rows = state.store.card_session_rows(card_id).ok()?;
    rows.into_iter().max_by(|a, b| a.created_at.cmp(&b.created_at)).map(|r| r.harness)
}

/// The gate/ship branch name for a card.
fn gate_branch(card_id: &str) -> String {
    let short: String = card_id.chars().take(10).collect::<String>().to_ascii_lowercase();
    format!("dapperflow/gate/{short}")
}

/// Compose the reviewer brief: the diff, acceptance criteria, plan pointer, and the
/// finding contract (`gate.md` / Adversarial review).
fn compose_reviewer_brief(ctx: &PipelineCtx, diff: &str) -> String {
    let acceptance = ctx
        .recipe
        .guidance_for(Stage::Verify)
        .map(str::to_string)
        .unwrap_or_default();
    format!(
        "You are the adversarial reviewer for a DapperFlow gate run. You are on a DIFFERENT \
         harness than the author on purpose.\n\n\
         Review the diff below against the card's acceptance criteria. For each real problem, \
         file a finding with `dflow finding add --severity <blocker|major|minor> --category \
         <mechanical|intent> --body \"<concrete failure scenario or rule citation>\"`. Use \
         `mechanical` for safe-mechanical issues (lint, formatting, dead imports, trivial test \
         fixes) that a fixer can apply automatically, and `intent` for anything touching \
         behavior, API shape, or scope. Do not file vibes; every finding needs a concrete \
         scenario. When done, exit.\n\n\
         ## Acceptance / verify guidance\n{acceptance}\n\n## Diff\n{diff}\n"
    )
}

/// Compose the fixer brief: the mechanical findings to apply and commit (`gate.md` / Autofix).
fn compose_fixer_brief(ctx: &PipelineCtx, findings: &[FindingRow]) -> String {
    let list: String = findings
        .iter()
        .enumerate()
        .map(|(i, f)| format!("{}. [{}] {}", i + 1, f.severity, f.body))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are the gate fixer for a DapperFlow gate run on card {card}. Apply ONLY the \
         safe-mechanical fixes below, then commit them (`git add -A && git commit`). Do not \
         change behavior, API shape, or scope. When done, exit.\n\n## Findings to fix\n{list}\n",
        card = ctx.card_id
    )
}

/// Raise (idempotently) the escalation Needs You for a gate run.
fn raise_gate_needs_you(state: &AppState, ctx: &PipelineCtx) {
    let score = needs_input_score(state, Some(&ctx.card_id));
    let _ = state.store.raise_needs_you(
        &ctx.card_id,
        NEEDS_YOU_GATE_FINDING,
        &format!("{NEEDS_YOU_GATE_FINDING}:{}", ctx.gate_run_id),
        score.max(70),
    );
}

// ---- ship helpers ----

/// Resolve `(card, project, latest gate run)` for a ship/merge, erroring when absent.
fn ship_context(
    state: &AppState,
    card_id: &str,
) -> Result<(dflow_proto::Card, dflow_proto::Project, GateRunRow), ProtocolError> {
    let card = state
        .store
        .get_card(card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("card {card_id}")))?;
    let project_id = card
        .project_id
        .clone()
        .ok_or_else(|| ProtocolError::bad_request("card has no project"))?;
    let project = state
        .store
        .get_project(&project_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("project {project_id}")))?;
    let run = state
        .store
        .latest_gate_run_for_card(card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::bad_request("no gate run for this card; run gate.run first"))?;
    Ok((card, project, run))
}

/// The filesystem path of a gate run's worktree.
fn gate_worktree_path(state: &AppState, run: &GateRunRow) -> Result<PathBuf, ProtocolError> {
    let wt_id = run
        .worktree_id
        .as_deref()
        .ok_or_else(|| ProtocolError::internal("gate run has no worktree"))?;
    let wt = state
        .store
        .get_worktree(wt_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("gate worktree {wt_id}")))?;
    Ok(PathBuf::from(wt.path))
}

/// Push a detached HEAD to `remote/<branch>` via the system git CLI (credential helper).
fn push_branch(cwd: &Path, remote: &str, branch: &str) -> Result<(), String> {
    git_capture(cwd, &["push", remote, &format!("HEAD:refs/heads/{branch}")]).map(|_| ())
}

/// The GitHub issue number for a `github_issue` origin card (`owner/repo#<n>`), else None.
fn issue_number_for(card: &dflow_proto::Card) -> Option<u64> {
    if card.origin_kind != "github_issue" {
        return None;
    }
    card.origin_ref.as_deref()?.rsplit('#').next()?.parse().ok()
}

/// The generated PR title.
fn pr_title(card: &dflow_proto::Card) -> String {
    card.title.trim().to_string()
}

/// Compose the PR body: a generated summary, a `Fixes #<n>` line for a GitHub-issue origin
/// card (so the issue closes on merge, `gate.md` / GitHub integration), and a card link.
fn compose_pr_body(card: &dflow_proto::Card, fixes: Option<u64>) -> String {
    let mut body = String::new();
    if let Some(brief) = card.brief.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        body.push_str(brief);
        body.push_str("\n\n");
    } else {
        body.push_str(&format!("{}\n\n", card.title.trim()));
    }
    if let Some(n) = fixes {
        body.push_str(&format!("Fixes #{n}\n\n"));
    }
    body.push_str(&format!("DapperFlow card: {}\n", card.id));
    body
}

/// Watch a PR's CI to settle via `gh pr checks`, streaming `ci_status` snapshots, bounded
/// by a timeout (`gate.md` / CI watch via `gh pr checks`).
struct CiOutcome {
    failing: bool,
}

fn watch_ci(state: &AppState, gh: &Gh, cwd: &Path, card_id: &str, pr_number: u64) -> CiOutcome {
    let timeout = Duration::from_millis(
        std::env::var("DFLOW_GATE_CI_TIMEOUT_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(30_000),
    );
    let deadline = Instant::now() + timeout;
    loop {
        let summary = match gh.pr_checks(cwd, pr_number) {
            Ok(s) => s,
            Err(_) => return CiOutcome { failing: false }, // treat a checks read error as "no CI"
        };
        let _ = state.store.append_card_event(
            card_id,
            dflow_core::event_kind::CI_STATUS,
            serde_json::json!({
                "pr_number": pr_number,
                "passing": summary.all_passing(),
                "pending": summary.pending(),
                "failing": summary.failing(),
                "checks": summary.checks.len(),
            }),
        );
        if summary.is_empty() || summary.all_passing() {
            return CiOutcome { failing: false };
        }
        if summary.failing() {
            return CiOutcome { failing: true };
        }
        if Instant::now() >= deadline {
            // Still pending at the deadline: do not block the merge forever, but do not
            // claim green either - report not-failing and let the merge proceed/parked by
            // gh itself (gh refuses to merge an un-green PR without --admin).
            return CiOutcome { failing: false };
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
}

/// Prove the shipped work landed: the PR reports merged (the squash-safe proof), or the
/// head sha is reachable from the default branch (`gate.md` / Teardown safety).
fn prove_landed(gh: &Gh, cwd: &Path, pr_number: u64, head_sha: Option<&str>, default_branch: &str) -> bool {
    if let Ok(pr) = gh.pr_view(cwd, pr_number) {
        if pr.state.eq_ignore_ascii_case("MERGED") || pr.merged_at.is_some() {
            return true;
        }
    }
    if let Some(head) = head_sha {
        // Fetch so the local default ref reflects the remote, then test ancestry.
        let _ = git_capture(cwd, &["fetch", "origin", default_branch]);
        for target in [format!("origin/{default_branch}"), default_branch.to_string()] {
            if git_capture(cwd, &["merge-base", "--is-ancestor", head, &target]).is_ok() {
                return true;
            }
        }
    }
    false
}

/// Return a gate worktree after a ship. When landed, reset it clean to the pool (warm
/// caches preserved); when not landed, refuse to reset - park it dirty with a Needs You,
/// so work is never silently discarded (`gate.md` / Teardown safety).
fn finalize_ship_worktree(
    state: &AppState,
    card: &dflow_proto::Card,
    project: &dflow_proto::Project,
    run: &GateRunRow,
    cwd: &Path,
    landed: bool,
) {
    let Some(wt_id) = run.worktree_id.as_deref() else { return };
    let _ = state.env_vault.cleanup(&state.store, &project.id, cwd);
    dflow_core::secret::registry().unregister(&format!("gate:{}", run.id));
    if landed {
        let _ = git_capture(cwd, &["checkout", "--force", "--detach", &project.default_branch]);
        let _ = git_capture(cwd, &["reset", "--hard"]);
        let _ = git_capture(cwd, &["clean", "-fd"]);
        let _ = state.store.set_worktree_lease(wt_id, dflow_core::lease_state::AVAILABLE, None);
        let _ = state.store.append_card_event(
            &card.id,
            dflow_core::event_kind::WORKTREE_RETURNED,
            serde_json::json!({ "worktree_id": wt_id, "outcome": "clean" }),
        );
    } else {
        let _ = state.store.set_worktree_lease(wt_id, dflow_core::lease_state::DIRTY, Some(&card.id));
        let score = needs_input_score(state, Some(&card.id));
        let _ = state.store.raise_needs_you(&card.id, "agent_blocked", &format!("worktree_unlanded:{wt_id}"), score.max(60));
        let _ = state.store.append_card_event(
            &card.id,
            dflow_core::event_kind::WORKTREE_RETURNED,
            serde_json::json!({ "worktree_id": wt_id, "outcome": "dirty", "reason": "ship landing not provable" }),
        );
    }
}

fn merge_method_str(m: MergeMethod) -> &'static str {
    match m {
        MergeMethod::Squash => "squash",
        MergeMethod::Merge => "merge",
        MergeMethod::Rebase => "rebase",
    }
}

/// `git checkout --force --detach <sha>` then reset/clean, so the gate worktree holds
/// exactly the commit under test.
fn checkout_head(cwd: &Path, head_sha: &str) -> Result<(), String> {
    git_capture(cwd, &["checkout", "--force", "--detach", head_sha]).map(|_| ())?;
    git_capture(cwd, &["reset", "--hard", head_sha]).map(|_| ())?;
    git_capture(cwd, &["clean", "-fd"]).map(|_| ())?;
    Ok(())
}

/// Run a shell command string in `cwd` with `env` merged, returning (success, code, output).
fn run_shell(cwd: &Path, cmd: &str, env: &BTreeMap<String, String>) -> (bool, i32, String) {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/d", "/c", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    command.current_dir(cwd);
    for (k, v) in env {
        command.env(k, v);
    }
    match command.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            (out.status.success(), out.status.code().unwrap_or(-1), text)
        }
        Err(e) => (false, -1, format!("could not run check: {e}")),
    }
}

/// Run a git subcommand in `cwd`, returning stdout on success or a message on failure.
fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("git not runnable: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Replace known secret values with the redaction marker before evidence is stored
/// (`security.md` / Gate evidence and check output).
fn scrub(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        if !s.is_empty() {
            out = out.replace(s, dflow_core::REDACTED);
        }
    }
    out
}

/// Sanitize a check name for use in an evidence file name.
fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

fn strictness_str(s: GateStrictness) -> &'static str {
    match s {
        GateStrictness::Full => "full",
        GateStrictness::ChecksOnly => "checks_only",
        GateStrictness::None => "none",
    }
}

/// Normalize a human escalation decision to a stored resolution.
fn normalize_resolution(input: &str) -> Result<&'static str, ProtocolError> {
    match input.trim().to_ascii_lowercase().as_str() {
        "accepted" | "accept" | "approve" | "approved" => Ok(resolution::ACCEPTED),
        "fixed" | "fix" => Ok(resolution::FIXED),
        "skipped" | "skip" => Ok(resolution::SKIPPED),
        other => Err(ProtocolError::bad_request(format!(
            "resolution must be accepted|fixed|skipped, got '{other}'"
        ))),
    }
}

/// Map a stored gate run row to its wire twin.
pub fn gate_run_info(r: GateRunRow) -> GateRunInfo {
    GateRunInfo {
        id: r.id,
        card_id: r.card_id,
        worktree_id: r.worktree_id,
        step: r.step,
        status: r.status,
        gate_strictness: r.gate_strictness,
        author_harness: r.author_harness,
        reviewer_harness: r.reviewer_harness,
        head_sha: r.head_sha,
        branch: r.branch,
        pr_number: r.pr_number,
        pr_url: r.pr_url,
        started_at: r.started_at,
        ended_at: r.ended_at,
    }
}

/// Map a stored finding row to its wire twin.
pub fn finding_info(f: FindingRow) -> FindingInfo {
    FindingInfo {
        id: f.id,
        gate_run_id: f.gate_run_id,
        card_id: f.card_id,
        severity: f.severity,
        category: f.category,
        source: f.source,
        body: f.body,
        evidence: f.evidence,
        resolution: f.resolution,
        created_at: f.created_at,
        resolved_at: f.resolved_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_reason_distinguishes_a_timeout_from_a_clean_exit() {
        // A fixer we observed exit on its own without changing anything made no changes; one
        // we had to kill at the session timeout did not complete (`gate.md` / Autofix earned
        // claim). This is the reason branch the E2E stubs cannot exercise deterministically:
        // on Windows ConPTY every PTY stub is only ever observed as "not finished", so this
        // unit test pins both arms.
        assert_eq!(no_change_reason(true), "autofix made no changes");
        assert_eq!(no_change_reason(false), "fixer did not complete");
    }

    #[test]
    fn preview_tail_truncates_on_a_char_boundary() {
        assert_eq!(preview_tail("  short tail  "), "short tail");
        let long = "x".repeat(700);
        let p = preview_tail(&long);
        assert!(p.ends_with("..."), "a long tail is elided: {p}");
        assert!(p.len() <= 603, "the preview is bounded: {}", p.len());
        // A multibyte tail is never split mid-codepoint (would panic on a bad boundary).
        let multibyte = "é".repeat(400); // 800 bytes
        let pm = preview_tail(&multibyte);
        assert!(pm.ends_with("..."), "multibyte tail elided: {pm}");
    }
}
