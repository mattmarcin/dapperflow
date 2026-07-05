//! GitHub transport verbs: auth reporting and read-only issue import (`roadmap.md`
//! M5.1-2, `product.md` / Card sources: GitHub issue import).
//!
//! These verbs sit on the owner scope and drive the `dflow-core::github` `gh` wrapper.
//! Import reuses the origin-dedupe path the onboarding audit proved in M3
//! (`upsert_origin_card`): one issue, one card, keyed on
//! `UNIQUE(origin_kind, origin_ref)` with `origin_ref = owner/repo#<n>`; re-import
//! refreshes fields but respects local lane moves and never refiles a dismissed card.

use std::path::PathBuf;

use dflow_core::github::{Gh, GhError, Issue, IssueFilters, RepoInfo};
use dflow_core::{NewCard, OriginUpsert};
use dflow_proto::{
    GithubAuthResult, GithubAuthStatus, GithubImportResult, GithubIssueFilter, GithubIssueGet,
    GithubIssueGetResult, GithubIssueInfo, GithubIssuePreview, GithubIssuesImport,
    GithubIssuesImportResult, GithubIssuesPreview, GithubIssuesPreviewResult, ProtocolError,
};

use crate::api::store_err;
use crate::server::AppState;

/// The origin kind stamped on imported cards (`data-model.md` / cards.origin_kind).
pub const ORIGIN_GITHUB_ISSUE: &str = "github_issue";

/// `github.auth.status {}`: gh presence/auth, never an OAuth flow (`roadmap.md` M5.1).
pub fn github_auth_status(_state: &AppState, _req: GithubAuthStatus) -> Result<GithubAuthResult, ProtocolError> {
    let auth = Gh::from_env().auth_status();
    Ok(GithubAuthResult {
        present: auth.present,
        authenticated: auth.authenticated,
        account: auth.account,
        host: auth.host,
        repo: None,
    })
}

/// `github.issues.preview`: list issues that would be imported plus their dedupe status,
/// creating nothing (read-only).
pub fn github_issues_preview(
    state: &AppState,
    req: GithubIssuesPreview,
) -> Result<GithubIssuesPreviewResult, ProtocolError> {
    let path = project_path(state, &req.project_id)?;
    let gh = Gh::from_env();
    let repo = gh.repo_view(&path).map_err(gh_err)?;
    let issues = gh.issue_list(&path, &to_core_filters(&req.filter)).map_err(gh_err)?;

    let mut previews = Vec::with_capacity(issues.len());
    for issue in issues {
        let origin_ref = repo.repo_ref().issue_ref(issue.number);
        let (dedupe, existing_card_id) =
            match state.store.get_card_by_origin(ORIGIN_GITHUB_ISSUE, &origin_ref).map_err(store_err)? {
                None => ("new".to_string(), None),
                Some((card, true)) => ("dismissed".to_string(), Some(card.id)),
                Some((card, false)) => ("tracked".to_string(), Some(card.id)),
            };
        previews.push(GithubIssuePreview {
            number: issue.number,
            title: issue.title,
            labels: issue.labels,
            state: issue.state,
            url: issue.url,
            dedupe,
            existing_card_id,
        });
    }
    Ok(GithubIssuesPreviewResult { repo: repo.repo_ref().slug(), issues: previews })
}

/// `github.issues.import`: create/refresh origin cards for the matching issues.
pub fn github_issues_import(
    state: &AppState,
    req: GithubIssuesImport,
) -> Result<GithubIssuesImportResult, ProtocolError> {
    let path = project_path(state, &req.project_id)?;
    let gh = Gh::from_env();
    let repo = gh.repo_view(&path).map_err(gh_err)?;
    let issues = gh.issue_list(&path, &to_core_filters(&req.filter)).map_err(gh_err)?;

    let mut results = Vec::with_capacity(issues.len());
    for issue in issues {
        let new = issue_to_card(&repo, &issue, &req.project_id, req.dial_recipe.clone());
        let (card, outcome) = state.store.upsert_origin_card(new).map_err(store_err)?;
        results.push(GithubImportResult {
            number: issue.number,
            title: issue.title,
            card_id: card.id,
            outcome: outcome_str(outcome).to_string(),
        });
    }
    Ok(GithubIssuesImportResult { repo: repo.repo_ref().slug(), results })
}

/// `github.issue.get { card_id }`: the issue snapshot for an origin card's Issue tab.
pub fn github_issue_get(
    state: &AppState,
    req: GithubIssueGet,
) -> Result<GithubIssueGetResult, ProtocolError> {
    let card = state
        .store
        .get_card(&req.card_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("card {}", req.card_id)))?;
    if card.origin_kind != ORIGIN_GITHUB_ISSUE {
        return Err(ProtocolError::bad_request(format!(
            "card {} is not a GitHub issue card (origin_kind={})",
            req.card_id, card.origin_kind
        )));
    }
    // The Issue tab reads the generic origin_data snapshot; fall back to the card fields
    // (older imports) so the tab is never empty.
    let issue = match state.store.get_card_origin_data(&req.card_id).map_err(store_err)? {
        Some(json) => serde_json::from_str::<GithubIssueInfo>(&json)
            .map_err(|e| ProtocolError::internal(format!("corrupt origin_data: {e}")))?,
        None => {
            let (repo, number) = split_origin_ref(card.origin_ref.as_deref().unwrap_or_default());
            GithubIssueInfo {
                number,
                repo,
                title: card.title.clone(),
                body: card.brief.clone().unwrap_or_default(),
                labels: Vec::new(),
                assignees: Vec::new(),
                milestone: None,
                state: String::new(),
                url: String::new(),
            }
        }
    };
    Ok(GithubIssueGetResult { issue })
}

// ---- helpers ----

/// Resolve a project id to its checkout path (where `gh` runs).
fn project_path(state: &AppState, project_id: &str) -> Result<PathBuf, ProtocolError> {
    let project = state
        .store
        .get_project(project_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("project {project_id}")))?;
    Ok(PathBuf::from(project.path))
}

/// Map the wire issue filter to the core one.
fn to_core_filters(f: &GithubIssueFilter) -> IssueFilters {
    IssueFilters {
        assignee: f.assignee.clone(),
        labels: f.labels.clone(),
        milestone: f.milestone.clone(),
        numbers: f.numbers.clone(),
        state: f.state.clone(),
        limit: f.limit,
    }
}

/// Build a NewCard for an imported issue: origin-keyed, typed by label heuristics, with
/// the body as the brief and the full snapshot in origin_data for the Issue tab.
fn issue_to_card(
    repo: &RepoInfo,
    issue: &Issue,
    project_id: &str,
    dial_recipe: Option<String>,
) -> NewCard {
    let snapshot = GithubIssueInfo {
        number: issue.number,
        repo: repo.repo_ref().slug(),
        title: issue.title.clone(),
        body: issue.body.clone(),
        labels: issue.labels.clone(),
        assignees: issue.assignees.clone(),
        milestone: issue.milestone.clone(),
        state: issue.state.clone(),
        url: issue.url.clone(),
    };
    NewCard {
        project_id: Some(project_id.to_string()),
        card_type: card_type_from_labels(&issue.labels),
        title: issue.title.clone(),
        lane: "inbox".to_string(),
        dial_recipe,
        brief: Some(issue.body.clone()),
        priority: 0,
        origin_kind: ORIGIN_GITHUB_ISSUE.to_string(),
        origin_ref: Some(repo.repo_ref().issue_ref(issue.number)),
        origin_data: serde_json::to_string(&snapshot).ok(),
    }
}

/// Type an imported card from its labels (`product.md`: bug/feature label heuristics).
fn card_type_from_labels(labels: &[String]) -> String {
    let lower: Vec<String> = labels.iter().map(|l| l.to_ascii_lowercase()).collect();
    if lower.iter().any(|l| l.contains("bug") || l.contains("defect") || l.contains("regression")) {
        "bug".to_string()
    } else if lower.iter().any(|l| l.contains("feature") || l.contains("enhancement")) {
        "feature".to_string()
    } else if lower.iter().any(|l| l.contains("chore") || l.contains("refactor") || l.contains("docs")) {
        "chore".to_string()
    } else {
        "feature".to_string()
    }
}

fn outcome_str(outcome: OriginUpsert) -> &'static str {
    match outcome {
        OriginUpsert::Created => "created",
        OriginUpsert::Refreshed => "refreshed",
        OriginUpsert::Suppressed => "suppressed",
    }
}

/// Split an `owner/repo#<n>` origin ref into `(owner/repo, n)`.
fn split_origin_ref(origin_ref: &str) -> (String, u64) {
    match origin_ref.split_once('#') {
        Some((repo, n)) => (repo.to_string(), n.parse().unwrap_or(0)),
        None => (origin_ref.to_string(), 0),
    }
}

/// Map a gh transport error to a protocol error, keeping the "gh absent -> local only"
/// signal clear for the UI (`gate.md`: "absent-gh degrades to local-only").
pub fn gh_err(err: GhError) -> ProtocolError {
    match err {
        GhError::Missing => ProtocolError::unsupported(err.to_string()),
        GhError::NotAuthenticated => ProtocolError::unsupported(err.to_string()),
        other => ProtocolError::internal(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_type_from_labels_heuristics() {
        assert_eq!(card_type_from_labels(&["bug".into()]), "bug");
        assert_eq!(card_type_from_labels(&["Regression".into()]), "bug");
        assert_eq!(card_type_from_labels(&["enhancement".into()]), "feature");
        assert_eq!(card_type_from_labels(&["feature-request".into()]), "feature");
        assert_eq!(card_type_from_labels(&["docs".into()]), "chore");
        // No recognizable label defaults to feature.
        assert_eq!(card_type_from_labels(&["question".into()]), "feature");
        assert_eq!(card_type_from_labels(&[]), "feature");
        // A bug label wins over a feature one (crashes are bugs first).
        assert_eq!(card_type_from_labels(&["enhancement".into(), "bug".into()]), "bug");
    }

    #[test]
    fn split_origin_ref_parses_owner_repo_number() {
        assert_eq!(split_origin_ref("acme/web#42"), ("acme/web".to_string(), 42));
        assert_eq!(split_origin_ref("owner/name#1"), ("owner/name".to_string(), 1));
        // Malformed refs degrade to (whole, 0) rather than panicking.
        assert_eq!(split_origin_ref("no-hash"), ("no-hash".to_string(), 0));
    }
}
