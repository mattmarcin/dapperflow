//! The GitHub transport layer (`gate.md` / GitHub integration, `roadmap.md` M5).
//!
//! A thin, typed wrapper over the local `gh` CLI with `--json` output. This is the
//! gh-first boundary the 2026-07-04 user decision fixed (`gate.md`): **API operations**
//! (issue import, PR create/view/merge, CI check watching, repo metadata) go through
//! `gh`; **push** stays on the system git CLI with the user's credential helper and is
//! deliberately NOT modeled here (see `push` in the ship path). gh-first because its
//! users are already authenticated, token storage is the OS credential manager's, and it
//! removes an entire OAuth-app registration and device-flow UX from M5.
//!
//! `gh` is a feature-scoped dependency (`product.md` principle 1): detected via
//! `gh auth status`; when it is absent or unauthenticated, PR mode degrades cleanly to
//! local-only with a one-line setup pointer, signalled through [`GhAuth`] and the
//! structured [`GhError::Missing`] / [`GhError::NotAuthenticated`] variants.
//!
//! # Testability
//!
//! Every subcommand runs through the [`GhRunner`] seam. Production uses [`ProcessRunner`]
//! (which shells out to the real `gh`, or a stub named by the `DFLOW_GH` env var so a
//! daemon under test can point at a fake `gh` on disk); unit tests use an in-process
//! stub runner with canned `--json` payloads. The daemon and its integration tests share
//! the same `DFLOW_GH` seam, so the real subprocess path (arg construction, spawn, stdout
//! capture, JSON parse, non-zero-exit handling) is exercised end to end against a real
//! stub binary, not only mocked.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// The env var naming the `gh` program to run. The daemon and its integration tests set
/// it to a stub binary; unset it resolves to the real `gh` on PATH.
pub const GH_PROGRAM_ENV: &str = "DFLOW_GH";

/// A structured GitHub transport error (`gate.md`: "structured errors; absent-gh degrades
/// to local-only with a clear signal").
#[derive(Debug, thiserror::Error)]
pub enum GhError {
    /// The `gh` binary could not be spawned (not installed / not on PATH). PR mode
    /// degrades to local-only; the message carries the one-line setup pointer.
    #[error("the GitHub CLI `gh` is not installed or not on PATH; install it from https://cli.github.com and run `gh auth login`, or use a local_only project")]
    Missing,
    /// `gh` ran but reports no authenticated account. PR mode degrades to local-only.
    #[error("`gh` is installed but not authenticated; run `gh auth login`")]
    NotAuthenticated,
    /// A `gh` subcommand exited non-zero for a reason other than auth.
    #[error("`gh {args}` failed (exit {code}): {stderr}")]
    Command { args: String, code: String, stderr: String },
    /// `gh` succeeded but its `--json` output did not parse into the expected shape.
    #[error("could not parse `gh {args}` output: {source}")]
    Parse {
        args: String,
        #[source]
        source: serde_json::Error,
    },
}

/// The raw result of one `gh` invocation, before any success/exit-code interpretation.
#[derive(Debug, Clone)]
pub struct GhOutput {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// The seam every `gh` subcommand runs through. A `run` returns the raw process output
/// (including a non-zero exit) or [`GhError::Missing`] when the binary cannot be spawned
/// at all; higher-level methods interpret exit codes and parse JSON.
pub trait GhRunner: Send + Sync {
    fn run(&self, args: &[&str], cwd: Option<&Path>) -> Result<GhOutput, GhError>;
}

/// The production runner: shells out to the real `gh` (or the `DFLOW_GH`-named stub).
pub struct ProcessRunner {
    program: OsString,
}

impl ProcessRunner {
    /// A runner using the `DFLOW_GH` program when set, else `gh` on PATH.
    pub fn from_env() -> Self {
        let program = std::env::var_os(GH_PROGRAM_ENV)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| OsString::from("gh"));
        ProcessRunner { program }
    }

    /// A runner for an explicit program path (tests, packaging).
    pub fn new(program: impl Into<OsString>) -> Self {
        ProcessRunner { program: program.into() }
    }
}

impl GhRunner for ProcessRunner {
    fn run(&self, args: &[&str], cwd: Option<&Path>) -> Result<GhOutput, GhError> {
        let mut cmd = Command::new(&self.program);
        cmd.args(args);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        // gh must never open a browser or prompt inside an unattended daemon.
        cmd.env("GH_PROMPT_DISABLED", "1");
        cmd.env("GH_NO_UPDATE_NOTIFIER", "1");
        cmd.env("CLICOLOR", "0");
        let output = cmd.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                GhError::Missing
            } else {
                // A spawn failure for any other reason still means "no usable gh here".
                tracing::debug!(error = %e, "gh spawn failed");
                GhError::Missing
            }
        })?;
        Ok(GhOutput {
            success: output.status.success(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The result of `gh auth status` (`roadmap.md` M5.1: `github.auth.*` verbs report gh
/// presence/auth rather than running an OAuth flow).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GhAuth {
    /// Whether the `gh` binary is present and runnable.
    pub present: bool,
    /// Whether `gh` reports an authenticated account.
    pub authenticated: bool,
    /// The logged-in account login, when `gh` reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The host the account is on (e.g. `github.com`), when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl GhAuth {
    /// The "gh is usable for PR mode" predicate: present AND authenticated.
    pub fn usable(&self) -> bool {
        self.present && self.authenticated
    }

    /// The absent signal (no gh binary at all).
    fn absent() -> Self {
        GhAuth { present: false, authenticated: false, account: None, host: None }
    }
}

/// A `owner/name` repository reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    /// `owner/name`.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The `origin_ref` for an issue in this repo: `owner/name#<number>`
    /// (`data-model.md` / cards.origin_ref: github "owner/repo#123").
    pub fn issue_ref(&self, number: u64) -> String {
        format!("{}/{}#{}", self.owner, self.name, number)
    }
}

/// Repo metadata from `gh repo view` (owner, name, default branch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
}

impl RepoInfo {
    pub fn repo_ref(&self) -> RepoRef {
        RepoRef { owner: self.owner.clone(), name: self.name.clone() }
    }
}

/// A GitHub issue as imported (`product.md` / Card sources: GitHub issue import).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestone: Option<String>,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub url: String,
}

/// Filters for issue import (`product.md`: assignee, label, and milestone filters, or a
/// curated picker of explicit numbers; never an unfiltered firehose into Inbox).
#[derive(Debug, Clone, Default)]
pub struct IssueFilters {
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub milestone: Option<String>,
    /// A curated set of explicit issue numbers; when set, the filters above are ignored
    /// and each number is fetched with `gh issue view`.
    pub numbers: Vec<u64>,
    /// Issue state filter (`open` default); `all`/`closed` allowed.
    pub state: Option<String>,
    pub limit: Option<u32>,
}

/// A pull request as created/viewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub title: String,
    /// The PR's head branch name.
    #[serde(default, rename = "headRefName")]
    pub head_ref_name: String,
    /// The PR head commit sha, for teardown containment proofs (`gate.md` / Teardown
    /// safety: "PR merged and head contained").
    #[serde(default, rename = "headRefOid")]
    pub head_ref_oid: String,
    #[serde(default, rename = "mergedAt", skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
}

/// One CI check row from `gh pr checks --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrCheck {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
    /// `pass | fail | pending | skipping | cancel` (gh's rollup bucket).
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub link: String,
}

/// The rollup of a PR's CI checks (`gate.md`: CI status streams back onto the card; CI
/// failures can trigger one bounded autofix loop before escalating).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksSummary {
    pub checks: Vec<PrCheck>,
}

impl ChecksSummary {
    /// Every check settled in a passing/neutral bucket (pass or skipping), none pending.
    pub fn all_passing(&self) -> bool {
        !self.checks.is_empty()
            && self.checks.iter().all(|c| matches!(c.bucket.as_str(), "pass" | "skipping"))
    }

    /// Any check still running.
    pub fn pending(&self) -> bool {
        self.checks.iter().any(|c| c.bucket == "pending")
    }

    /// Any check that failed or was cancelled.
    pub fn failing(&self) -> bool {
        self.checks.iter().any(|c| matches!(c.bucket.as_str(), "fail" | "cancel"))
    }

    /// No checks configured at all (a repo with no CI): treated as "nothing to watch".
    pub fn is_empty(&self) -> bool {
        self.checks.is_empty()
    }
}

/// The merge method for `gh pr merge` (squash is the M5 default, `gate.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

impl MergeMethod {
    fn flag(self) -> &'static str {
        match self {
            MergeMethod::Squash => "--squash",
            MergeMethod::Merge => "--merge",
            MergeMethod::Rebase => "--rebase",
        }
    }
}

/// Parameters for opening a PR.
#[derive(Debug, Clone)]
pub struct PrCreate {
    pub title: String,
    pub body: String,
    pub base: String,
    pub head: String,
}

/// The typed `gh` transport.
pub struct Gh {
    runner: Box<dyn GhRunner>,
}

impl Gh {
    /// The production transport over the real `gh` (or the `DFLOW_GH` stub).
    pub fn from_env() -> Self {
        Gh { runner: Box::new(ProcessRunner::from_env()) }
    }

    /// A transport over an arbitrary runner (tests).
    pub fn with_runner(runner: Box<dyn GhRunner>) -> Self {
        Gh { runner }
    }

    /// `gh auth status`: presence + authentication, degrading cleanly when gh is absent
    /// (`roadmap.md` M5.1). Never an error - the absent/unauth states are data.
    pub fn auth_status(&self) -> GhAuth {
        let out = match self.runner.run(&["auth", "status"], None) {
            Ok(o) => o,
            Err(GhError::Missing) => return GhAuth::absent(),
            Err(_) => return GhAuth::absent(),
        };
        // gh writes auth status to stdout (recent) or stderr (older); scan both.
        let text = format!("{}\n{}", out.stdout, out.stderr);
        let (account, host) = parse_auth_identity(&text);
        GhAuth { present: true, authenticated: out.success, account, host }
    }

    /// `gh repo view --json name,owner,defaultBranchRef`: owner/name/default-branch for
    /// the repo at `cwd` (`gate.md`: "repo view for owner/name/default-branch").
    pub fn repo_view(&self, cwd: &Path) -> Result<RepoInfo, GhError> {
        let args = ["repo", "view", "--json", "name,owner,defaultBranchRef"];
        let value = self.run_json(&args, Some(cwd))?;
        let owner = value
            .get("owner")
            .and_then(|o| o.get("login"))
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string();
        let name = value.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
        let default_branch = value
            .get("defaultBranchRef")
            .and_then(|d| d.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("main")
            .to_string();
        Ok(RepoInfo { owner, name, default_branch })
    }

    /// `gh issue list --json ...` with the given filters, or `gh issue view` per number
    /// when a curated number set is supplied (`product.md` / issue import filters).
    pub fn issue_list(&self, cwd: &Path, filters: &IssueFilters) -> Result<Vec<Issue>, GhError> {
        if !filters.numbers.is_empty() {
            let mut issues = Vec::with_capacity(filters.numbers.len());
            for n in &filters.numbers {
                issues.push(self.issue_view(cwd, *n)?);
            }
            return Ok(issues);
        }
        let fields = "number,title,body,labels,assignees,milestone,state,url";
        let mut args: Vec<String> =
            ["issue", "list", "--json", fields].iter().map(|s| s.to_string()).collect();
        if let Some(a) = filters.assignee.as_deref().filter(|s| !s.is_empty()) {
            args.push("--assignee".into());
            args.push(a.into());
        }
        for label in &filters.labels {
            if !label.is_empty() {
                args.push("--label".into());
                args.push(label.clone());
            }
        }
        if let Some(m) = filters.milestone.as_deref().filter(|s| !s.is_empty()) {
            args.push("--milestone".into());
            args.push(m.into());
        }
        args.push("--state".into());
        args.push(filters.state.clone().unwrap_or_else(|| "open".into()));
        args.push("--limit".into());
        args.push(filters.limit.unwrap_or(50).to_string());
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let value = self.run_json(&arg_refs, Some(cwd))?;
        parse_issue_array(&value, &arg_refs)
    }

    /// `gh issue view <n> --json ...`.
    pub fn issue_view(&self, cwd: &Path, number: u64) -> Result<Issue, GhError> {
        let n = number.to_string();
        let fields = "number,title,body,labels,assignees,milestone,state,url";
        let args = ["issue", "view", n.as_str(), "--json", fields];
        let value = self.run_json(&args, Some(cwd))?;
        parse_issue(&value)
            .ok_or_else(|| GhError::Parse { args: args.join(" "), source: shape_error() })
    }

    /// `gh pr create --title --body --base --head`. gh prints the PR URL on stdout; the
    /// number is parsed from it, then `pr_view` fills in head sha/state.
    pub fn pr_create(&self, cwd: &Path, req: &PrCreate) -> Result<PrInfo, GhError> {
        let args =
            ["pr", "create", "--title", &req.title, "--body", &req.body, "--base", &req.base, "--head", &req.head];
        let out = self.run_ok(&args, Some(cwd))?;
        let url = out
            .stdout
            .lines()
            .map(str::trim)
            .find(|l| l.contains("/pull/"))
            .unwrap_or_else(|| out.stdout.trim())
            .to_string();
        let number = parse_pr_number(&url).ok_or_else(|| GhError::Parse {
            args: "pr create".into(),
            source: shape_error(),
        })?;
        // Fill in head sha/state/branch for the ship path's teardown proofs.
        match self.pr_view(cwd, number) {
            Ok(mut info) => {
                if info.url.is_empty() {
                    info.url = url;
                }
                Ok(info)
            }
            Err(_) => Ok(PrInfo {
                number,
                url,
                state: "OPEN".into(),
                title: req.title.clone(),
                head_ref_name: req.head.clone(),
                head_ref_oid: String::new(),
                merged_at: None,
            }),
        }
    }

    /// `gh pr view <n> --json ...`.
    pub fn pr_view(&self, cwd: &Path, number: u64) -> Result<PrInfo, GhError> {
        let n = number.to_string();
        let fields = "number,url,state,title,headRefName,headRefOid,mergedAt";
        let args = ["pr", "view", n.as_str(), "--json", fields];
        let value = self.run_json(&args, Some(cwd))?;
        serde_json::from_value(value).map_err(|e| GhError::Parse { args: args.join(" "), source: e })
    }

    /// `gh pr checks <n> --json name,state,bucket,link`. gh exits non-zero when checks
    /// are pending or failing, so the JSON is parsed regardless of exit code; only a
    /// spawn/auth failure is an error (`gate.md`: CI watch via `gh pr checks`).
    pub fn pr_checks(&self, cwd: &Path, number: u64) -> Result<ChecksSummary, GhError> {
        let n = number.to_string();
        let args = ["pr", "checks", n.as_str(), "--json", "name,state,bucket,link"];
        let out = self.runner.run(&args, Some(cwd))?;
        self.reject_if_auth(&args, &out)?;
        // "no checks reported on this ref" exits 1 with empty stdout: treat as no checks.
        let body = out.stdout.trim();
        if body.is_empty() {
            return Ok(ChecksSummary { checks: Vec::new() });
        }
        let checks: Vec<PrCheck> = serde_json::from_str(body)
            .map_err(|e| GhError::Parse { args: args.join(" "), source: e })?;
        Ok(ChecksSummary { checks })
    }

    /// `gh pr merge <n> --squash [--delete-branch]` (`gate.md`: merge with squash default).
    pub fn pr_merge(
        &self,
        cwd: &Path,
        number: u64,
        method: MergeMethod,
        delete_branch: bool,
    ) -> Result<(), GhError> {
        let n = number.to_string();
        let mut args = vec!["pr", "merge", n.as_str(), method.flag()];
        if delete_branch {
            args.push("--delete-branch");
        }
        self.run_ok(&args, Some(cwd))?;
        Ok(())
    }

    // ---- internals ----

    /// Run a subcommand, map auth/exit failures to structured errors, return raw output.
    fn run_ok(&self, args: &[&str], cwd: Option<&Path>) -> Result<GhOutput, GhError> {
        let out = self.runner.run(args, cwd)?;
        self.reject_if_auth(args, &out)?;
        if !out.success {
            return Err(GhError::Command {
                args: args.join(" "),
                code: out.code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
                stderr: out.stderr.trim().to_string(),
            });
        }
        Ok(out)
    }

    /// Run a subcommand expected to emit JSON on stdout and parse it into a `Value`.
    fn run_json(&self, args: &[&str], cwd: Option<&Path>) -> Result<serde_json::Value, GhError> {
        let out = self.run_ok(args, cwd)?;
        serde_json::from_str(out.stdout.trim())
            .map_err(|e| GhError::Parse { args: args.join(" "), source: e })
    }

    /// Map an authentication failure (any subcommand) to [`GhError::NotAuthenticated`].
    fn reject_if_auth(&self, _args: &[&str], out: &GhOutput) -> Result<(), GhError> {
        if !out.success && looks_like_auth_failure(&out.stderr) {
            return Err(GhError::NotAuthenticated);
        }
        Ok(())
    }
}

/// Whether a `gh` stderr indicates an authentication problem (so the caller degrades to
/// local-only rather than surfacing a raw command failure).
fn looks_like_auth_failure(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("gh auth login")
        || s.contains("not logged in")
        || s.contains("authentication required")
        || s.contains("requires authentication")
        || s.contains("no git remotes found") && s.contains("auth")
}

/// Parse the account/host out of `gh auth status` text (best-effort).
///
/// Recent gh: "✓ Logged in to github.com account octocat (keyring)". Older gh:
/// "✓ Logged in to github.com as octocat (...)".
fn parse_auth_identity(text: &str) -> (Option<String>, Option<String>) {
    let mut account = None;
    let mut host = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.split("Logged in to ").nth(1) {
            let mut it = rest.split_whitespace();
            if let Some(h) = it.next() {
                host = Some(h.trim_end_matches(&[',', ':'][..]).to_string());
            }
            // "account <name>" (new) or "as <name>" (old).
            let mut prev = "";
            for tok in rest.split_whitespace() {
                if (prev == "account" || prev == "as") && account.is_none() {
                    account = Some(tok.trim_matches(&['(', ')', ',', '.'][..]).to_string());
                    break;
                }
                prev = tok;
            }
        }
    }
    (account, host)
}

/// Parse the trailing `/pull/<n>` number out of a PR URL.
fn parse_pr_number(url: &str) -> Option<u64> {
    let tail = url.rsplit("/pull/").next()?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Parse a JSON array of issues (`gh issue list`).
fn parse_issue_array(value: &serde_json::Value, args: &[&str]) -> Result<Vec<Issue>, GhError> {
    let arr = value.as_array().ok_or_else(|| GhError::Parse {
        args: args.join(" "),
        source: shape_error(),
    })?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        if let Some(issue) = parse_issue(item) {
            out.push(issue);
        }
    }
    Ok(out)
}

/// Parse one issue object from gh's `--json` shape (labels/assignees/milestone nested).
fn parse_issue(value: &serde_json::Value) -> Option<Issue> {
    let number = value.get("number")?.as_u64()?;
    let title = value.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let body = value.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string();
    let labels = value
        .get("labels")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let assignees = value
        .get("assignees")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("login").and_then(|n| n.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let milestone = value
        .get("milestone")
        .and_then(|m| m.get("title"))
        .and_then(|t| t.as_str())
        .map(str::to_string);
    let state = value.get("state").and_then(|s| s.as_str()).unwrap_or_default().to_string();
    let url = value.get("url").and_then(|u| u.as_str()).unwrap_or_default().to_string();
    Some(Issue { number, title, body, labels, assignees, milestone, state, url })
}

/// A synthetic serde error for shape mismatches that are not raw parse failures.
fn shape_error() -> serde_json::Error {
    serde::de::Error::custom("unexpected gh --json shape")
}
