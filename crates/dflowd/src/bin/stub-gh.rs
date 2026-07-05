//! A fake `gh` for the M5 integration tests (`gate.md` / GitHub integration).
//!
//! The daemon runs the real gh transport (`dflow-core::github`) but points `DFLOW_GH`
//! at this binary, so tests exercise the true subprocess path - arg construction, spawn,
//! stdout capture, `--json` parsing, and exit-code handling - against a real `gh`-shaped
//! program, not an in-process mock. Behaviour is driven entirely by env vars set on the
//! daemon (which flow through to this child); state (the PR a `pr create` opened, its
//! merged status) persists across invocations in a JSON file under `DFLOW_STUB_GH_STATE`
//! so `pr create -> pr view -> pr checks -> pr merge` is coherent.
//!
//! Env contract:
//! - `DFLOW_STUB_GH_AUTHED` = "1" (default) | "0"     -> auth status result
//! - `DFLOW_STUB_GH_ACCOUNT` (default "tester")
//! - `DFLOW_STUB_GH_REPO` = "owner/name/branch" (default "acme/web/main")
//! - `DFLOW_STUB_GH_ISSUES` = path to a JSON array file for `issue list`/`issue view`
//! - `DFLOW_STUB_GH_CHECKS` = path to a JSON array file for `pr checks` (default 1 pass)
//! - `DFLOW_STUB_GH_STATE` = directory for PR state + a record of `pr create`/`pr merge`
//!   args (so a test can assert the generated PR body carried `Fixes #<n>`)

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match a.as_slice() {
        ["auth", "status", ..] => auth_status(),
        ["repo", "view", ..] => repo_view(),
        ["issue", "list", ..] => issue_list(),
        ["issue", "view", n, ..] => issue_view(n),
        ["pr", "create", ..] => pr_create(&args),
        ["pr", "view", n, ..] => pr_view(n),
        ["pr", "checks", n, ..] => pr_checks(n),
        ["pr", "merge", n, ..] => pr_merge(n, &args),
        _ => {
            eprintln!("stub-gh: unhandled args: {args:?}");
            2
        }
    };
    std::process::exit(code);
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn state_dir() -> Option<PathBuf> {
    env("DFLOW_STUB_GH_STATE").map(PathBuf::from)
}

fn print(s: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(s.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

fn auth_status() -> i32 {
    let authed = env("DFLOW_STUB_GH_AUTHED").as_deref() != Some("0");
    let account = env("DFLOW_STUB_GH_ACCOUNT").unwrap_or_else(|| "tester".into());
    if authed {
        // gh writes auth status to stdout on recent versions.
        print(&format!("github.com\n  Logged in to github.com account {account} (keyring)"));
        0
    } else {
        eprintln!("You are not logged in to any GitHub hosts. To log in, run: gh auth login");
        1
    }
}

fn repo() -> (String, String, String) {
    let spec = env("DFLOW_STUB_GH_REPO").unwrap_or_else(|| "acme/web/main".into());
    let parts: Vec<&str> = spec.split('/').collect();
    let owner = parts.first().copied().unwrap_or("acme").to_string();
    let name = parts.get(1).copied().unwrap_or("web").to_string();
    let branch = parts.get(2).copied().unwrap_or("main").to_string();
    (owner, name, branch)
}

fn repo_view() -> i32 {
    let (owner, name, branch) = repo();
    print(&format!(
        r#"{{"name":"{name}","owner":{{"login":"{owner}"}},"defaultBranchRef":{{"name":"{branch}"}},"nameWithOwner":"{owner}/{name}"}}"#
    ));
    0
}

fn issues() -> serde_json::Value {
    match env("DFLOW_STUB_GH_ISSUES").and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(text) => serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!([])),
        None => serde_json::json!([]),
    }
}

fn issue_list() -> i32 {
    print(&issues().to_string());
    0
}

fn issue_view(n: &str) -> i32 {
    let want: u64 = n.parse().unwrap_or(0);
    if let Some(arr) = issues().as_array() {
        if let Some(found) = arr.iter().find(|i| i.get("number").and_then(|x| x.as_u64()) == Some(want)) {
            print(&found.to_string());
            return 0;
        }
    }
    eprintln!("stub-gh: could not find issue #{n}");
    1
}

fn pr_number() -> u64 {
    env("DFLOW_STUB_GH_PR_NUMBER").and_then(|s| s.parse().ok()).unwrap_or(42)
}

/// The head sha of the checkout gh was invoked in (the real gate worktree HEAD), so the
/// teardown containment proof runs against real git objects.
fn head_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn pr_create(args: &[String]) -> i32 {
    let (owner, name, _b) = repo();
    let number = pr_number();
    let head = flag_value(args, "--head").unwrap_or("dapperflow/gate").to_string();
    let title = flag_value(args, "--title").unwrap_or("").to_string();
    let body = flag_value(args, "--body").unwrap_or("").to_string();
    let url = format!("https://github.com/{owner}/{name}/pull/{number}");
    let pr = serde_json::json!({
        "number": number,
        "url": url,
        "state": "OPEN",
        "title": title,
        "headRefName": head,
        "headRefOid": head_sha(),
        "mergedAt": serde_json::Value::Null,
    });
    if let Some(dir) = state_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("pr.json"), pr.to_string());
        // Record the create args so a test can assert the generated body carried Fixes #n.
        let _ = std::fs::write(dir.join("pr_create_body.txt"), &body);
        let _ = std::fs::write(dir.join("pr_create_args.txt"), args.join("\n"));
    }
    print(&url);
    0
}

fn load_pr() -> serde_json::Value {
    state_dir()
        .and_then(|d| std::fs::read_to_string(d.join("pr.json")).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| {
            let (owner, name, _) = repo();
            let number = pr_number();
            serde_json::json!({
                "number": number,
                "url": format!("https://github.com/{owner}/{name}/pull/{number}"),
                "state": "OPEN",
                "title": "",
                "headRefName": "dapperflow/gate",
                "headRefOid": head_sha(),
                "mergedAt": serde_json::Value::Null,
            })
        })
}

fn pr_view(_n: &str) -> i32 {
    print(&load_pr().to_string());
    0
}

fn pr_checks(_n: &str) -> i32 {
    let checks: serde_json::Value =
        match env("DFLOW_STUB_GH_CHECKS").and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => serde_json::from_str(&text).unwrap_or_else(|_| default_checks()),
            None => default_checks(),
        };
    print(&checks.to_string());
    // gh exits non-zero when any bucket is not pass/skipping.
    let all_ok = checks
        .as_array()
        .map(|a| a.iter().all(|c| matches!(c.get("bucket").and_then(|b| b.as_str()), Some("pass") | Some("skipping"))))
        .unwrap_or(false);
    if all_ok {
        0
    } else {
        1
    }
}

fn default_checks() -> serde_json::Value {
    serde_json::json!([{ "name": "build", "state": "SUCCESS", "bucket": "pass", "link": "" }])
}

fn pr_merge(_n: &str, args: &[String]) -> i32 {
    let mut pr = load_pr();
    pr["state"] = serde_json::Value::String("MERGED".into());
    pr["mergedAt"] = serde_json::Value::String("2026-07-05T00:00:00Z".into());
    if let Some(dir) = state_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("pr.json"), pr.to_string());
        let _ = std::fs::write(dir.join("pr_merge_args.txt"), args.join("\n"));
    }
    print("Merged pull request");
    0
}
