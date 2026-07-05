//! The single daemon data directory and everything under it.
//!
//! All daemon state lives under one data dir (`architecture.md` / worktree pool,
//! `data-model.md` / Files on disk): the runtime file, the single-instance lock,
//! the SQLite store, the scrollback rings, and the worktree pool root.
//!
//! `DFLOW_DATA_DIR` overrides the location *everywhere*. Tests and dev daemons set
//! it to an isolated temp dir so they never collide with a live user daemon
//! (deliverable 2). When it is unset the default is the per-user app-data dir
//! (`%LOCALAPPDATA%\DapperFlow` on Windows, `~/.local/share/dapperflow` elsewhere).

use std::path::{Path, PathBuf};

/// Errors resolving the data directory.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("no data directory: {0}")]
    NoDataDir(String),
    #[error("creating {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The resolved data directory and the paths derived from it.
#[derive(Debug, Clone)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Resolve the data dir from the environment without creating anything.
    ///
    /// `DFLOW_DATA_DIR`, if set, is used verbatim (it points *at* the data dir, with
    /// no extra suffix). Otherwise the per-user default is used.
    pub fn resolve() -> Result<DataDir, PathError> {
        if let Some(dir) = std::env::var_os("DFLOW_DATA_DIR") {
            let dir = PathBuf::from(dir);
            if dir.as_os_str().is_empty() {
                return Err(PathError::NoDataDir("DFLOW_DATA_DIR is set but empty".into()));
            }
            return Ok(DataDir { root: dir });
        }
        Ok(DataDir { root: default_root()? })
    }

    /// Wrap an explicit directory (used by tests and embedders).
    pub fn at(root: impl Into<PathBuf>) -> DataDir {
        DataDir { root: root.into() }
    }

    /// Resolve from the environment and create the directory tree.
    pub fn ensure() -> Result<DataDir, PathError> {
        let dir = Self::resolve()?;
        dir.create_all()?;
        Ok(dir)
    }

    /// Create the data dir and its standard subdirectories.
    pub fn create_all(&self) -> Result<(), PathError> {
        for path in
            [self.root().to_path_buf(), self.scrollback_dir(), self.worktrees_dir(), self.hooks_dir()]
        {
            std::fs::create_dir_all(&path)
                .map_err(|source| PathError::Create { path: path.clone(), source })?;
        }
        Ok(())
    }

    /// The data-dir root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The single-instance lock file.
    pub fn lock_file(&self) -> PathBuf {
        self.root.join("dflowd.lock")
    }

    /// The published runtime descriptor (`{ port, token, pid, version }`).
    pub fn runtime_file(&self) -> PathBuf {
        self.root.join("runtime.json")
    }

    /// The SQLite store file.
    pub fn db_file(&self) -> PathBuf {
        self.root.join("store.db")
    }

    /// The scrollback-ring directory (`<data-dir>/scrollback/<session-ulid>.ring`).
    pub fn scrollback_dir(&self) -> PathBuf {
        self.root.join("scrollback")
    }

    /// The scrollback ring for one session.
    pub fn scrollback_file(&self, session_id: &str) -> PathBuf {
        self.scrollback_dir().join(format!("{session_id}.ring"))
    }

    /// The worktree pool root (`<data-dir>/worktrees/<project-slug>/<slot>`).
    pub fn worktrees_dir(&self) -> PathBuf {
        self.root.join("worktrees")
    }

    /// The directory for session-scoped harness settings files (`<data-dir>/hooks`),
    /// e.g. the Claude Code `--settings` file wiring lifecycle hooks to the daemon.
    pub fn hooks_dir(&self) -> PathBuf {
        self.root.join("hooks")
    }

    /// The user-scoped recipe directory (`<app-data>/recipes/`, `recipes.md` /
    /// Resolution and scoping). Created lazily by `recipe.install`, not at startup.
    pub fn recipes_dir(&self) -> PathBuf {
        self.root.join("recipes")
    }

    /// A card's artifact directory (`data-model.md` / Files on disk:
    /// `<app-data>/cards/<card-ulid>/artifacts/`). Created lazily by `artifact.register`.
    pub fn card_artifacts_dir(&self, card_id: &str) -> PathBuf {
        self.root.join("cards").join(card_id).join("artifacts")
    }

    /// The served HTML file for one artifact version (`<artifacts>/<doc-id>.html`).
    pub fn artifact_file(&self, card_id: &str, doc_id: &str) -> PathBuf {
        self.card_artifacts_dir(card_id).join(format!("{doc_id}.html"))
    }
}

/// The per-user default data dir when `DFLOW_DATA_DIR` is unset.
fn default_root() -> Result<PathBuf, PathError> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| PathError::NoDataDir("LOCALAPPDATA is not set".into()))?;
        Ok(base.join("DapperFlow"))
    }
    #[cfg(not(windows))]
    {
        if let Some(base) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
            return Ok(base.join("dapperflow"));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| PathError::NoDataDir("HOME is not set".into()))?;
        Ok(home.join(".local").join("share").join("dapperflow"))
    }
}

/// A filesystem-safe slug for a project pool directory (`<project-slug>`).
///
/// Lowercased, non-alphanumerics collapsed to single dashes, trimmed, and suffixed
/// with a short id fragment so two projects with the same basename never collide.
pub fn project_slug(name: &str, id: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let base = slug.trim_matches('-');
    let base = if base.is_empty() { "project" } else { base };
    // A ULID is 26 chars; its tail is plenty to disambiguate identical names.
    let frag: String = id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    let frag = frag.to_ascii_lowercase();
    if frag.is_empty() {
        base.to_string()
    } else {
        format!("{base}-{frag}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_dir_derives_subpaths() {
        let dir = DataDir::at("C:/tmp/dflow-test");
        assert!(dir.db_file().ends_with("store.db"));
        assert!(dir.runtime_file().ends_with("runtime.json"));
        assert!(dir.lock_file().ends_with("dflowd.lock"));
        assert!(dir.scrollback_file("01ABC").ends_with("01ABC.ring"));
        assert!(dir.worktrees_dir().ends_with("worktrees"));
    }

    #[test]
    fn slug_is_filesystem_safe_and_disambiguated() {
        let a = project_slug("Acme Web!", "01HZZZZZZZZZZZZZZZZZZABCDEF");
        assert!(a.starts_with("acme-web-"));
        assert!(!a.contains(' '));
        assert!(!a.contains('!'));
        // Same name, different id -> different slug tail.
        let b = project_slug("Acme Web!", "01HZZZZZZZZZZZZZZZZZZUVWXYZ");
        assert_ne!(a, b);
    }

    #[test]
    fn empty_name_falls_back() {
        let s = project_slug("", "01HZZZZZZZZZZZZZZZZZZABCDEF");
        assert!(s.starts_with("project-"));
    }
}
