//! Worktree pool tests against a real scratch git repo the test creates.
//!
//! These shell out to system git (present on the dev machine and CI), exercising
//! the real lease/return/dirty paths rather than a mock.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dflow_proto::Project;

use super::{ReleaseOutcome, WorktreePool};
use crate::store::{lease_state, NewCard, Store};

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-wt-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Create a scratch repo with one commit on `main`; return its path.
fn scratch_repo(base: &Path) -> PathBuf {
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git(&repo, &["init", "-b", "main"]);
    run_git(&repo, &["config", "user.name", "DapperFlow Test"]);
    run_git(&repo, &["config", "user.email", "test@dapperflow.local"]);
    std::fs::write(repo.join("README.md"), "scratch\n").unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-m", "init"]);
    repo
}

/// A pool + project + card scaffold sharing one temp base.
fn scaffold(tag: &str) -> (PathBuf, Arc<Store>, WorktreePool, Project, String) {
    let base = unique_dir(tag);
    let repo = scratch_repo(&base);
    let store = Arc::new(Store::open(&base.join("store.db")).unwrap());
    let project = store
        .add_project(&repo.to_string_lossy(), "Scratch", "main", "local_only")
        .unwrap();
    let card = store
        .create_card(NewCard { project_id: Some(project.id.clone()), title: "Work".into(), ..Default::default() })
        .unwrap();
    let pool = WorktreePool::new(Arc::clone(&store), base.join("worktrees"));
    (base, store, pool, project, card.id)
}

#[test]
fn lease_creates_a_detached_worktree() {
    let (_base, store, pool, project, card_id) = scaffold("lease");
    let wt = pool.lease(&project, &card_id).unwrap();

    assert!(Path::new(&wt.path).is_dir(), "worktree dir should exist");
    // A linked worktree has a `.git` file (not a dir) pointing at the common dir.
    assert!(Path::new(&wt.path).join(".git").exists(), "should be a git worktree");
    assert_eq!(wt.lease_state, lease_state::LEASED);
    assert_eq!(wt.leased_by_card.as_deref(), Some(card_id.as_str()));

    // worktree_leased event landed on the card.
    let events = store.card_events(&card_id, None, 50).unwrap();
    assert!(events.iter().any(|e| e.kind == "worktree_leased"));
}

#[test]
fn clean_release_returns_to_available_and_can_be_reused() {
    let (_base, store, pool, project, card_id) = scaffold("clean");
    let wt = pool.lease(&project, &card_id).unwrap();

    let outcome = pool.release(&wt.id).unwrap();
    assert_eq!(outcome, ReleaseOutcome::Clean);
    let row = store.get_worktree(&wt.id).unwrap().unwrap();
    assert_eq!(row.lease_state, lease_state::AVAILABLE);
    assert!(row.leased_by_card.is_none());
    assert!(store.card_events(&card_id, None, 50).unwrap().iter().any(|e| e.kind == "worktree_returned"));

    // Leasing again reuses the same slot (warm-cache reuse).
    let wt2 = pool.lease(&project, &card_id).unwrap();
    assert_eq!(wt2.id, wt.id, "should reuse the returned slot");
    assert_eq!(store.worktrees_for_project(&project.id).unwrap().len(), 1);
}

#[test]
fn uncommitted_changes_park_the_worktree_dirty() {
    let (_base, store, pool, project, card_id) = scaffold("dirty-uncommitted");
    let wt = pool.lease(&project, &card_id).unwrap();

    // An untracked file is unlanded work.
    std::fs::write(Path::new(&wt.path).join("scratch.txt"), "wip\n").unwrap();

    let outcome = pool.release(&wt.id).unwrap();
    match outcome {
        ReleaseOutcome::Dirty { reasons } => assert!(!reasons.is_empty()),
        other => panic!("expected Dirty, got {other:?}"),
    }
    let row = store.get_worktree(&wt.id).unwrap().unwrap();
    assert_eq!(row.lease_state, lease_state::DIRTY);
    // A Needs You item was raised, and the untracked file survives.
    assert!(!store.list_needs_you(true).unwrap().is_empty());
    assert!(Path::new(&wt.path).join("scratch.txt").exists(), "dirty work must never be discarded");
    assert!(store.card_events(&card_id, None, 50).unwrap().iter().any(|e| e.kind == "needs_you_raised"));
}

#[test]
fn unlanded_commit_parks_the_worktree_dirty() {
    let (_base, store, pool, project, card_id) = scaffold("dirty-commit");
    let wt = pool.lease(&project, &card_id).unwrap();
    let wt_path = PathBuf::from(&wt.path);

    // A commit that never reached the default branch or any remote is unlanded.
    std::fs::write(wt_path.join("feature.txt"), "done\n").unwrap();
    run_git(&wt_path, &["add", "-A"]);
    run_git(&wt_path, &["commit", "-m", "feature work"]);

    let outcome = pool.release(&wt.id).unwrap();
    match outcome {
        ReleaseOutcome::Dirty { reasons } => {
            assert!(reasons.iter().any(|r| r.contains("landed")), "reasons: {reasons:?}");
        }
        other => panic!("expected Dirty, got {other:?}"),
    }
    assert_eq!(store.get_worktree(&wt.id).unwrap().unwrap().lease_state, lease_state::DIRTY);
}
