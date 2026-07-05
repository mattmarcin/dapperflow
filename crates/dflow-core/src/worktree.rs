//! The worktree pool (`architecture.md` / worktree pool).
//!
//! Per project the daemon keeps a pool directory
//! `<data-dir>/worktrees/<project-slug>/<slot>`. Leasing reuses a clean available
//! slot or creates one with `git worktree add --detach`. Returning refuses when
//! there is unlanded work (uncommitted changes, or commits on no remote branch):
//! such a worktree is marked `dirty` and a Needs You item is raised, so work is
//! never silently discarded. Otherwise the worktree is reset clean and marked
//! available, preserving warm caches.
//!
//! Implementation shells out to system git (a hard dependency of the workflow); no
//! libgit2, to avoid worktree edge-case divergence (`architecture.md`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use dflow_proto::Project;

use crate::paths::project_slug;
use crate::store::{event_kind, lease_state, Store, StoreError, WorktreeRow};

/// Errors from the worktree pool.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git {op} failed ({code}): {stderr}")]
    Git { op: String, code: String, stderr: String },
    #[error("git not runnable: {0}")]
    GitSpawn(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
}

/// The outcome of returning a leased worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// Reset clean and marked available.
    Clean,
    /// Unlanded work found: parked `dirty`, Needs You raised, nothing discarded.
    Dirty { reasons: Vec<String> },
}

/// The worktree pool for one daemon/data-dir.
pub struct WorktreePool {
    store: Arc<Store>,
    root: PathBuf,
}

impl WorktreePool {
    /// Create a pool rooted at `root` (`<data-dir>/worktrees`).
    pub fn new(store: Arc<Store>, root: PathBuf) -> Self {
        WorktreePool { store, root }
    }

    /// Lease a worktree for `card_id` in `project` (the `pooled` strategy,
    /// `recipes.md` / implement.worktree).
    ///
    /// Reuses the lowest-slot available worktree whose directory still exists, else
    /// creates a new detached worktree. Appends `worktree_leased` to the card.
    pub fn lease(&self, project: &Project, card_id: &str) -> Result<WorktreeRow, WorktreeError> {
        if let Some(available) = self.store.find_available_worktree(&project.id)? {
            if Path::new(&available.path).is_dir() {
                self.store.set_worktree_lease(&available.id, lease_state::LEASED, Some(card_id))?;
                self.store.append_card_event(
                    card_id,
                    event_kind::WORKTREE_LEASED,
                    serde_json::json!({
                        "worktree_id": available.id,
                        "slot": available.slot,
                        "path": available.path,
                        "reused": true,
                    }),
                )?;
                return self
                    .store
                    .get_worktree(&available.id)?
                    .ok_or_else(|| WorktreeError::NotFound(format!("worktree {}", available.id)));
            }
            // The directory is gone (manual cleanup?); retire the stale row.
            self.store.set_worktree_lease(&available.id, lease_state::RETIRED, None)?;
        }

        self.lease_new_slot(project, card_id)
    }

    /// Lease an always-new worktree for `card_id` (the `fresh` strategy,
    /// `recipes.md` / implement.worktree): never reuses a pool slot, so the checkout
    /// starts with no warm caches and no history from prior leases. The slot still
    /// joins the pool afterward like any other (return semantics are unchanged).
    pub fn lease_fresh(&self, project: &Project, card_id: &str) -> Result<WorktreeRow, WorktreeError> {
        self.lease_new_slot(project, card_id)
    }

    /// Create and lease a brand-new pool slot with `git worktree add --detach`.
    fn lease_new_slot(&self, project: &Project, card_id: &str) -> Result<WorktreeRow, WorktreeError> {
        let slot = self.store.next_worktree_slot(&project.id)?;
        let slug = project_slug(&project.name, &project.id);
        let slot_dir = self.root.join(&slug).join(slot.to_string());
        if let Some(parent) = slot_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `git worktree add --detach` requires the slot dir not exist yet.
        git(
            Path::new(&project.path),
            "worktree add",
            &["worktree", "add", "--detach", &slot_dir.to_string_lossy()],
        )?;
        let wt = self.store.insert_worktree(
            &project.id,
            slot,
            &slot_dir.to_string_lossy(),
            lease_state::LEASED,
            Some(card_id),
            None,
        )?;
        self.store.append_card_event(
            card_id,
            event_kind::WORKTREE_LEASED,
            serde_json::json!({
                "worktree_id": wt.id,
                "slot": wt.slot,
                "path": wt.path,
                "reused": false,
            }),
        )?;
        Ok(wt)
    }

    /// Return a leased worktree.
    ///
    /// If there is unlanded work the worktree is parked `dirty`, a Needs You item is
    /// raised on its card, and nothing is reset. Otherwise it is reset clean (warm
    /// caches preserved) and marked available, appending `worktree_returned`.
    pub fn release(&self, worktree_id: &str) -> Result<ReleaseOutcome, WorktreeError> {
        let wt = self
            .store
            .get_worktree(worktree_id)?
            .ok_or_else(|| WorktreeError::NotFound(format!("worktree {worktree_id}")))?;
        let project = self
            .store
            .get_project(&wt.project_id)?
            .ok_or_else(|| WorktreeError::NotFound(format!("project {}", wt.project_id)))?;
        let path = PathBuf::from(&wt.path);

        let reasons = self.unlanded_reasons(&path, &project.default_branch);
        if !reasons.is_empty() {
            self.store.set_worktree_lease(
                worktree_id,
                lease_state::DIRTY,
                wt.leased_by_card.as_deref(),
            )?;
            if let Some(card_id) = &wt.leased_by_card {
                // Score v0: a parked worktree outranks idle ideas but not a live block.
                self.store.raise_needs_you(
                    card_id,
                    "agent_blocked",
                    &format!("worktree_dirty:{worktree_id}"),
                    60,
                )?;
                self.store.append_card_event(
                    card_id,
                    event_kind::WORKTREE_RETURNED,
                    serde_json::json!({
                        "worktree_id": worktree_id,
                        "outcome": "dirty",
                        "reasons": reasons,
                    }),
                )?;
            }
            return Ok(ReleaseOutcome::Dirty { reasons });
        }

        // Clean: drop tracked changes and untracked files, but keep ignored caches
        // (node_modules, target, .next) so a reused slot stays warm (`architecture.md`).
        git(&path, "reset", &["reset", "--hard"])?;
        git(&path, "clean", &["clean", "-fd"])?;
        self.store.set_worktree_lease(worktree_id, lease_state::AVAILABLE, None)?;
        if let Some(card_id) = &wt.leased_by_card {
            self.store.append_card_event(
                card_id,
                event_kind::WORKTREE_RETURNED,
                serde_json::json!({ "worktree_id": worktree_id, "outcome": "clean" }),
            )?;
        }
        Ok(ReleaseOutcome::Clean)
    }

    /// Reasons a worktree has unlanded work, empty when it is safe to reset.
    ///
    /// Conservative: if landed-ness cannot be verified, the worktree is treated as
    /// dirty rather than risking discarding work. Submodule and LFS edge cases are a
    /// documented v0 gap (`architecture.md` / dirty classification).
    fn unlanded_reasons(&self, path: &Path, default_branch: &str) -> Vec<String> {
        let mut reasons = Vec::new();

        match git(path, "status", &["status", "--porcelain"]) {
            Ok(out) if !out.trim().is_empty() => {
                reasons.push("uncommitted or untracked changes".to_string());
            }
            Ok(_) => {}
            Err(e) => reasons.push(format!("could not read status: {e}")),
        }

        // Commits on HEAD reachable from neither the default branch nor any remote.
        match git(
            path,
            "rev-list",
            &["rev-list", "--count", "HEAD", "--not", default_branch, "--remotes"],
        ) {
            Ok(out) => {
                if out.trim().parse::<u64>().unwrap_or(1) > 0 {
                    reasons.push("commits not landed on the default branch or any remote".to_string());
                }
            }
            Err(e) => reasons.push(format!("could not check for unlanded commits: {e}")),
        }

        reasons
    }
}

/// Run a git subcommand in `cwd`, returning stdout on success.
fn git(cwd: &Path, op: &str, args: &[&str]) -> Result<String, WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|e| WorktreeError::GitSpawn(e.to_string()))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(WorktreeError::Git {
            op: op.to_string(),
            code: output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into()),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests;
