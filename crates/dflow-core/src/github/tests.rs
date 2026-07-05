//! Unit tests for the `gh` transport (`gate.md` / GitHub integration).
//!
//! Every subcommand is exercised through an in-process [`StubRunner`] with canned
//! `--json` payloads, proving arg construction and JSON parsing without a real `gh`.
//! The real-subprocess path (a fake `gh` binary on disk) is proven separately by the
//! `dflowd` integration tests via the `DFLOW_GH` seam.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::*;

/// A canned response keyed by a substring that must appear in the joined args.
struct Canned {
    contains: Vec<&'static str>,
    out: GhOutput,
}

/// An in-process runner returning canned outputs and recording the args it saw.
struct StubRunner {
    canned: Vec<Canned>,
    seen: Mutex<Vec<String>>,
    missing: bool,
}

impl StubRunner {
    fn new(canned: Vec<Canned>) -> Self {
        StubRunner { canned, seen: Mutex::new(Vec::new()), missing: false }
    }

    fn missing() -> Self {
        StubRunner { canned: Vec::new(), seen: Mutex::new(Vec::new()), missing: true }
    }

    fn last_args(&self) -> String {
        self.seen.lock().unwrap().last().cloned().unwrap_or_default()
    }
}

impl GhRunner for StubRunner {
    fn run(&self, args: &[&str], _cwd: Option<&Path>) -> Result<GhOutput, GhError> {
        if self.missing {
            return Err(GhError::Missing);
        }
        let joined = args.join(" ");
        self.seen.lock().unwrap().push(joined.clone());
        for c in &self.canned {
            if c.contains.iter().all(|needle| joined.contains(needle)) {
                return Ok(c.out.clone());
            }
        }
        panic!("no canned gh response for args: {joined}");
    }
}

/// `Arc<StubRunner>` is itself a runner, so a test can hand one clone to `Gh` and keep
/// another to assert on the args that were recorded (no `unsafe`, no raw pointers).
impl GhRunner for Arc<StubRunner> {
    fn run(&self, args: &[&str], cwd: Option<&Path>) -> Result<GhOutput, GhError> {
        (**self).run(args, cwd)
    }
}

fn ok(stdout: &str) -> GhOutput {
    GhOutput { success: true, code: Some(0), stdout: stdout.to_string(), stderr: String::new() }
}

fn fail(code: i32, stdout: &str, stderr: &str) -> GhOutput {
    GhOutput { success: false, code: Some(code), stdout: stdout.to_string(), stderr: stderr.to_string() }
}

fn cwd() -> PathBuf {
    PathBuf::from(".")
}

#[test]
fn auth_status_absent_when_gh_missing() {
    let gh = Gh::with_runner(Box::new(StubRunner::missing()));
    let auth = gh.auth_status();
    assert!(!auth.present);
    assert!(!auth.authenticated);
    assert!(!auth.usable());
}

#[test]
fn auth_status_authenticated_parses_account_and_host() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["auth", "status"],
        out: ok("github.com\n  ✓ Logged in to github.com account octocat (keyring)\n"),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let auth = gh.auth_status();
    assert!(auth.present && auth.authenticated && auth.usable());
    assert_eq!(auth.account.as_deref(), Some("octocat"));
    assert_eq!(auth.host.as_deref(), Some("github.com"));
}

#[test]
fn auth_status_present_but_unauthenticated() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["auth", "status"],
        out: fail(1, "", "You are not logged in to any GitHub hosts. Run gh auth login to authenticate."),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let auth = gh.auth_status();
    assert!(auth.present);
    assert!(!auth.authenticated);
    assert!(!auth.usable());
}

#[test]
fn repo_view_extracts_owner_name_default_branch() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["repo", "view", "--json"],
        out: ok(r#"{"name":"acme-web","owner":{"login":"acme"},"defaultBranchRef":{"name":"main"}}"#),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let info = gh.repo_view(&cwd()).unwrap();
    assert_eq!(info.owner, "acme");
    assert_eq!(info.name, "acme-web");
    assert_eq!(info.default_branch, "main");
    assert_eq!(info.repo_ref().slug(), "acme/acme-web");
    assert_eq!(info.repo_ref().issue_ref(7), "acme/acme-web#7");
}

#[test]
fn issue_list_parses_filters_and_nested_fields() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["issue", "list", "--json"],
        out: ok(r#"[
          {"number":12,"title":"Login 500","body":"repro steps","state":"OPEN","url":"https://github.com/acme/web/issues/12",
           "labels":[{"name":"bug"},{"name":"p1"}],"assignees":[{"login":"alice"}],"milestone":{"title":"v1"}},
          {"number":15,"title":"Add export","body":"","state":"OPEN","url":"https://github.com/acme/web/issues/15",
           "labels":[{"name":"feature"}],"assignees":[],"milestone":null}
        ]"#),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let filters = IssueFilters {
        assignee: Some("alice".into()),
        labels: vec!["bug".into()],
        milestone: Some("v1".into()),
        state: Some("open".into()),
        limit: Some(25),
        ..Default::default()
    };
    let issues = gh.issue_list(&cwd(), &filters).unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].number, 12);
    assert_eq!(issues[0].labels, vec!["bug", "p1"]);
    assert_eq!(issues[0].assignees, vec!["alice"]);
    assert_eq!(issues[0].milestone.as_deref(), Some("v1"));
    assert_eq!(issues[1].milestone, None);
}

#[test]
fn issue_list_by_numbers_uses_issue_view() {
    let runner = StubRunner::new(vec![
        Canned {
            contains: vec!["issue", "view", "12"],
            out: ok(r#"{"number":12,"title":"Login 500","body":"b","state":"OPEN","url":"u","labels":[],"assignees":[],"milestone":null}"#),
        },
        Canned {
            contains: vec!["issue", "view", "9"],
            out: ok(r#"{"number":9,"title":"Slow page","body":"b","state":"OPEN","url":"u","labels":[],"assignees":[],"milestone":null}"#),
        },
    ]);
    let gh = Gh::with_runner(Box::new(runner));
    let filters = IssueFilters { numbers: vec![12, 9], ..Default::default() };
    let issues = gh.issue_list(&cwd(), &filters).unwrap();
    assert_eq!(issues.iter().map(|i| i.number).collect::<Vec<_>>(), vec![12, 9]);
}

#[test]
fn pr_create_parses_url_then_views_head() {
    let runner = StubRunner::new(vec![
        Canned {
            contains: vec!["pr", "create"],
            out: ok("https://github.com/acme/web/pull/42\n"),
        },
        Canned {
            contains: vec!["pr", "view", "42"],
            out: ok(r#"{"number":42,"url":"https://github.com/acme/web/pull/42","state":"OPEN","title":"Fix login","headRefName":"dapperflow/gate/x","headRefOid":"deadbeef","mergedAt":null}"#),
        },
    ]);
    let gh = Gh::with_runner(Box::new(runner));
    let info = gh
        .pr_create(
            &cwd(),
            &PrCreate {
                title: "Fix login".into(),
                body: "Fixes #12".into(),
                base: "main".into(),
                head: "dapperflow/gate/x".into(),
            },
        )
        .unwrap();
    assert_eq!(info.number, 42);
    assert_eq!(info.head_ref_oid, "deadbeef");
    assert_eq!(info.head_ref_name, "dapperflow/gate/x");
}

#[test]
fn pr_checks_rollup_buckets() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["pr", "checks", "42"],
        // gh exits non-zero when anything is pending/failing; we parse regardless.
        out: fail(
            1,
            r#"[{"name":"build","state":"SUCCESS","bucket":"pass","link":"l"},
                {"name":"test","state":"IN_PROGRESS","bucket":"pending","link":"l"}]"#,
            "",
        ),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let summary = gh.pr_checks(&cwd(), 42).unwrap();
    assert!(!summary.all_passing());
    assert!(summary.pending());
    assert!(!summary.failing());
}

#[test]
fn pr_checks_all_pass() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["pr", "checks", "42"],
        out: ok(r#"[{"name":"build","state":"SUCCESS","bucket":"pass","link":"l"}]"#),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let summary = gh.pr_checks(&cwd(), 42).unwrap();
    assert!(summary.all_passing());
    assert!(!summary.pending() && !summary.failing());
}

#[test]
fn pr_checks_empty_when_no_ci() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["pr", "checks"],
        out: fail(1, "", "no checks reported on the 'main' branch"),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let summary = gh.pr_checks(&cwd(), 42).unwrap();
    assert!(summary.is_empty());
    assert!(!summary.all_passing()); // empty is not "passing"
}

#[test]
fn pr_merge_squash_builds_expected_args() {
    let runner = Arc::new(StubRunner::new(vec![Canned {
        contains: vec!["pr", "merge", "42", "--squash", "--delete-branch"],
        out: ok(""),
    }]));
    let gh = Gh::with_runner(Box::new(Arc::clone(&runner)));
    gh.pr_merge(&cwd(), 42, MergeMethod::Squash, true).unwrap();
    let seen = runner.last_args();
    assert!(seen.contains("pr merge 42 --squash --delete-branch"), "{seen}");
}

#[test]
fn command_failure_surfaces_structured_error() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["pr", "view", "999"],
        out: fail(1, "", "could not find pull request #999"),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let err = gh.pr_view(&cwd(), 999).unwrap_err();
    match err {
        GhError::Command { stderr, .. } => assert!(stderr.contains("could not find")),
        other => panic!("expected Command error, got {other:?}"),
    }
}

#[test]
fn auth_failure_on_any_subcommand_maps_to_not_authenticated() {
    let runner = StubRunner::new(vec![Canned {
        contains: vec!["issue", "list"],
        out: fail(1, "", "To get started with GitHub CLI, please run: gh auth login"),
    }]);
    let gh = Gh::with_runner(Box::new(runner));
    let err = gh.issue_list(&cwd(), &IssueFilters::default()).unwrap_err();
    assert!(matches!(err, GhError::NotAuthenticated), "got {err:?}");
}

#[test]
fn issue_list_arg_construction_includes_all_filters() {
    let runner = Arc::new(StubRunner::new(vec![Canned { contains: vec!["issue", "list"], out: ok("[]") }]));
    let gh = Gh::with_runner(Box::new(Arc::clone(&runner)));
    let filters = IssueFilters {
        assignee: Some("bob".into()),
        labels: vec!["bug".into(), "p1".into()],
        milestone: Some("v2".into()),
        state: Some("all".into()),
        limit: Some(10),
        ..Default::default()
    };
    gh.issue_list(&cwd(), &filters).unwrap();
    let seen = runner.last_args();
    assert!(seen.contains("--assignee bob"), "{seen}");
    assert!(seen.contains("--label bug"), "{seen}");
    assert!(seen.contains("--label p1"), "{seen}");
    assert!(seen.contains("--milestone v2"), "{seen}");
    assert!(seen.contains("--state all"), "{seen}");
    assert!(seen.contains("--limit 10"), "{seen}");
}
