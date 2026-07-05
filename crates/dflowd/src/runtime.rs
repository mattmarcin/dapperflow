//! Daemon runtime file and single-instance lock (`security.md` / Token
//! architecture, `architecture.md` / Platform notes).
//!
//! All daemon state lives under one data dir resolved by `dflow_core::DataDir`,
//! which honors `DFLOW_DATA_DIR` so tests and dev daemons never collide with a live
//! user daemon. On first run the daemon mints a root bearer token and stores it,
//! with an owner-only ACL, in `<data-dir>/runtime.json` alongside the loopback port.
//! A lock file held for the process lifetime enforces a single instance per data
//! dir; if the lock is already held the daemon exits with a clear message rather than
//! ever interfering with the running instance.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use dflow_core::DataDir;
use fs2::FileExt;
use rand::Rng;
use serde::{Deserialize, Serialize};

/// The published runtime descriptor the desktop app reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    /// Loopback port the WS server is listening on (OS-assigned each run).
    pub port: u16,
    /// Root bearer token (persisted across runs; rotatable in a later phase).
    pub token: String,
    /// The daemon process id.
    pub pid: u32,
    /// Daemon build string.
    pub version: String,
}

/// A held daemon runtime: the single-instance lock plus the resolved data dir.
pub struct Runtime {
    data_dir: DataDir,
    token: String,
    version: String,
    // The lock file must stay open for the whole process lifetime; the OS releases
    // it when the process exits, so there is never a stale lock to clean up.
    _lock: File,
}

impl Runtime {
    /// Acquire the single-instance lock and recover or mint the root token.
    /// Errors if another daemon instance already holds the lock for this data dir.
    pub fn acquire(version: &str) -> Result<Runtime> {
        let data_dir = DataDir::ensure().context("resolving data dir")?;

        let lock_path = data_dir.lock_file();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("opening lock file {}", lock_path.display()))?;
        lock.try_lock_exclusive().map_err(|_| {
            anyhow!(
                "another dflowd instance is already running for data dir {} (lock held)",
                data_dir.root().display()
            )
        })?;

        let token = recover_or_mint_token(&data_dir)?;

        Ok(Runtime { data_dir, token, version: version.to_string(), _lock: lock })
    }

    /// The root bearer token.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The resolved data directory.
    pub fn data_dir(&self) -> &DataDir {
        &self.data_dir
    }

    /// The data-dir root (for logging).
    pub fn dir(&self) -> &Path {
        self.data_dir.root()
    }

    /// Publish the runtime file for the given listening port and lock it down to
    /// the current user.
    pub fn publish(&self, port: u16) -> Result<RuntimeInfo> {
        let info = RuntimeInfo {
            port,
            token: self.token.clone(),
            pid: std::process::id(),
            version: self.version.clone(),
        };
        let path = self.data_dir.runtime_file();
        let json = serde_json::to_string_pretty(&info)?;
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
        restrict_to_owner(&path);
        Ok(info)
    }

    /// Remove the runtime file on clean shutdown so clients do not read a stale port.
    pub fn cleanup(&self) {
        let _ = fs::remove_file(self.data_dir.runtime_file());
    }
}

/// Read the token from an existing runtime file, or mint a new one.
fn recover_or_mint_token(data_dir: &DataDir) -> Result<String> {
    let path = data_dir.runtime_file();
    if let Ok(mut file) = File::open(&path) {
        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_ok() {
            if let Ok(info) = serde_json::from_str::<RuntimeInfo>(&buf) {
                if !info.token.is_empty() {
                    return Ok(info.token);
                }
            }
        }
    }
    Ok(mint_token())
}

/// Mint a 48-character alphanumeric bearer token.
fn mint_token() -> String {
    let mut rng = rand::rng();
    (0..48).map(|_| rng.sample(rand::distr::Alphanumeric) as char).collect()
}

/// Restrict a file to the current user only, via `icacls`.
///
/// `security.md` requires the token file to carry an owner-only ACL. Phase 0 shells
/// out to `icacls` (simple, but not skipped). Failure is logged loudly rather than
/// aborting the daemon, since the file still lives under the user's profile.
fn restrict_to_owner(path: &PathBuf) {
    let user = match current_user() {
        Some(u) => u,
        None => {
            tracing::error!("could not determine current user; runtime file ACL NOT restricted");
            return;
        }
    };
    let result = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{user}:(F)"))
        .output();
    match result {
        Ok(out) if out.status.success() => {
            tracing::debug!(?path, %user, "restricted runtime file to owner");
        }
        Ok(out) => {
            tracing::error!(
                status = ?out.status,
                stderr = %String::from_utf8_lossy(&out.stderr),
                "icacls failed to restrict runtime file"
            );
        }
        Err(err) => tracing::error!(%err, "could not run icacls to restrict runtime file"),
    }
}

/// The current user as `DOMAIN\user`, via `whoami`.
fn current_user() -> Option<String> {
    let out = Command::new("whoami").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}
