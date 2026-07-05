//! The env vault: per-project encrypted store plus worktree materialization
//! (`environments.md`).
//!
//! [`EnvVault`] owns the credential-store backend ([`crypto::VaultCrypto`]) and drives
//! the whole lifecycle against a [`Store`]:
//!
//! - `set`/`list`/`delete` - encrypt a value and persist it; list names and kinds only.
//! - `materialize` - decrypt a project's entries into a leased worktree: `var`/`secret`
//!   entries become an env map merged into the spawn environment, `file` entries are
//!   written to their relative target paths under the worktree.
//! - `cleanup` - shred materialized secret files (overwrite then delete) before the
//!   worktree re-enters the pool, so a secret never rides a warm-cached checkout.
//! - `import` - parse an existing `.env` file, classify each key, and ingest it.
//!
//! The plaintext value never touches the store; only ciphertext is persisted, and only
//! `materialize` ever unseals (inside the daemon, never toward a client).

pub mod crypto;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::store::env::{env_kind, EnvEntryMeta};
use crate::store::{Store, StoreError};

pub use crypto::{CryptoError, VaultCrypto};

/// Errors from the vault.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid: {0}")]
    Invalid(String),
}

/// The result of materializing a project's vault into a worktree.
///
/// `env` carries the `var` and `secret` entries to merge into the spawn environment.
/// `secret_values` carries the plaintext of every `secret` entry plus the content of
/// every `file` entry, so the daemon can register them with the scrubber for the
/// spawned session. `files`/`file_targets` record what was written for the
/// `env_materialized` event and for teardown shredding - never any value.
#[derive(Debug, Default)]
pub struct MaterializedEnv {
    /// `var` and `secret` entries as `KEY -> value`, for the spawn env.
    pub env: BTreeMap<String, String>,
    /// Plaintext values that must be scrubbed from durable captures for this session
    /// (`secret` values and `file` contents).
    pub secret_values: Vec<String>,
    /// Absolute paths of the `file` entries written into the worktree.
    pub files: Vec<PathBuf>,
    /// Relative target paths of the `file` entries (evidence for the event, no values).
    pub file_targets: Vec<String>,
    /// Counts for the `env_materialized` event payload.
    pub vars: usize,
    pub secrets: usize,
}

/// One materialized `file` entry that drifted from its vault value at worktree return
/// (`environments.md` / Drift guard). The summary is **value-masked**: only key names
/// are carried, never any value (`security.md` / Drift guard captures show diffs with
/// values masked). Absorbing a drift is a later `env.set` from the UI; this is the raise
/// side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftEntry {
    /// The relative target path of the drifted `file` entry.
    pub target: String,
    /// Keys present on disk but not in the vault value (the agent added them).
    pub added_keys: Vec<String>,
    /// Keys in the vault value but no longer on disk (the agent removed them).
    pub removed_keys: Vec<String>,
    /// Keys whose value changed (names only, values masked).
    pub changed_keys: Vec<String>,
    /// The file is not `KEY=VALUE`-parseable, so only a byte-level change is known.
    pub opaque_change: bool,
}

/// What an import did (`env.import`).
#[derive(Debug, Default)]
pub struct ImportReport {
    /// `(key, kind)` for every entry ingested, in file order.
    pub entries: Vec<(String, String)>,
    pub vars: usize,
    pub secrets: usize,
    /// Lines that could not be parsed as `KEY=VALUE` (blank/comment lines are silently
    /// ignored; these are surfaced so the user sees what was skipped).
    pub skipped: Vec<String>,
}

impl ImportReport {
    /// Total entries ingested.
    pub fn imported(&self) -> usize {
        self.entries.len()
    }
}

/// The per-project env vault.
pub struct EnvVault {
    crypto: Box<dyn VaultCrypto>,
}

impl Default for EnvVault {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvVault {
    /// A vault using this platform's default credential-store backend.
    pub fn new() -> Self {
        Self { crypto: crypto::default_crypto() }
    }

    /// A vault with an explicit backend (tests inject a fake).
    pub fn with_crypto(crypto: Box<dyn VaultCrypto>) -> Self {
        Self { crypto }
    }

    /// Whether the backing credential store provides real at-rest encryption (false on
    /// the non-Windows developer stub).
    pub fn crypto_is_secure(&self) -> bool {
        self.crypto.is_secure()
    }

    /// `env.set`: encrypt `value` and upsert the entry. `set` is write-only from
    /// clients - the returned metadata carries no value. A `file` entry requires a
    /// non-empty `target`; `var`/`secret` must not carry one.
    pub fn set_entry(
        &self,
        store: &Store,
        project_id: &str,
        key: &str,
        kind: &str,
        value: &str,
        target: Option<&str>,
    ) -> Result<EnvEntryMeta, EnvError> {
        let key = key.trim();
        if key.is_empty() {
            return Err(EnvError::Invalid("env key must not be empty".into()));
        }
        if !env_kind::is_known(kind) {
            return Err(EnvError::Invalid(format!("unknown env kind '{kind}' (var|secret|file)")));
        }
        let target = target.map(str::trim).filter(|t| !t.is_empty());
        if kind == env_kind::FILE && target.is_none() {
            return Err(EnvError::Invalid("a file entry needs a relative target path".into()));
        }
        if kind != env_kind::FILE && target.is_some() {
            return Err(EnvError::Invalid(format!("kind '{kind}' does not take a target path")));
        }
        if let Some(t) = target {
            validate_target(t)?;
        }
        let ciphertext = self.crypto.seal(value.as_bytes())?;
        Ok(store.set_env_entry(project_id, key, kind, target, &ciphertext, self.crypto.key_id())?)
    }

    /// `env.list`: names and kinds only, never values.
    pub fn list_entries(&self, store: &Store, project_id: &str) -> Result<Vec<EnvEntryMeta>, EnvError> {
        Ok(store.list_env_entries(project_id)?)
    }

    /// Delete a vault entry. Returns whether it existed.
    pub fn delete_entry(&self, store: &Store, project_id: &str, key: &str) -> Result<bool, EnvError> {
        Ok(store.delete_env_entry(project_id, key)?)
    }

    /// `env.materialize`: decrypt a project's entries into `worktree_path`.
    ///
    /// `var`/`secret` entries become the returned `env` map; `file` entries are written
    /// to `worktree_path/<target>` (parent dirs created), overwriting any prior copy.
    /// The plaintext of secrets and file contents is returned in `secret_values` for the
    /// scrubber, never persisted. A blob whose `key_id` does not match this backend is
    /// skipped with a warning rather than failing the whole dispatch (a vault sealed on
    /// another OS cannot be opened here).
    pub fn materialize(
        &self,
        store: &Store,
        project_id: &str,
        worktree_path: &Path,
    ) -> Result<MaterializedEnv, EnvError> {
        let mut out = MaterializedEnv::default();
        for entry in store.env_entries_sealed(project_id)? {
            if entry.key_id != self.crypto.key_id() {
                tracing::warn!(
                    key = %entry.key,
                    sealed_by = %entry.key_id,
                    backend = %self.crypto.key_id(),
                    "vault entry sealed by a different credential store; skipping materialization"
                );
                continue;
            }
            let value = String::from_utf8_lossy(&self.crypto.open(&entry.ciphertext)?).into_owned();
            match entry.kind.as_str() {
                env_kind::VAR => {
                    out.env.insert(entry.key.clone(), value);
                    out.vars += 1;
                }
                env_kind::SECRET => {
                    if !value.is_empty() {
                        out.secret_values.push(value.clone());
                    }
                    out.env.insert(entry.key.clone(), value);
                    out.secrets += 1;
                }
                env_kind::FILE => {
                    let target = match entry.target.as_deref() {
                        Some(t) if !t.is_empty() => t,
                        _ => {
                            tracing::warn!(key = %entry.key, "file vault entry has no target; skipping");
                            continue;
                        }
                    };
                    validate_target(target)?;
                    let abs = worktree_path.join(target);
                    if let Some(parent) = abs.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&abs, value.as_bytes())?;
                    if !value.is_empty() {
                        // File contents are sensitive (service-account json, .dev.vars);
                        // scrub their verbatim value from durable captures too.
                        out.secret_values.push(value);
                    }
                    out.file_targets.push(target.to_string());
                    out.files.push(abs);
                }
                other => {
                    tracing::warn!(key = %entry.key, kind = %other, "unknown vault kind; skipping");
                }
            }
        }
        Ok(out)
    }

    /// Materialize only the `var`/`secret` entries into an env map, writing no files.
    ///
    /// Used by an `in_place` dispatch, whose "worktree" is the user's real project
    /// checkout: injecting env vars is safe, but writing (and later shredding) `file`
    /// entries there would clobber and then delete the user's own files, so files are
    /// skipped. `file` entries are reported in `file_targets` as skipped-for-in-place so
    /// the event still records that they were withheld.
    pub fn materialize_env_only(
        &self,
        store: &Store,
        project_id: &str,
    ) -> Result<MaterializedEnv, EnvError> {
        let mut out = MaterializedEnv::default();
        for entry in store.env_entries_sealed(project_id)? {
            if entry.key_id != self.crypto.key_id() {
                continue;
            }
            match entry.kind.as_str() {
                env_kind::VAR => {
                    out.env.insert(entry.key.clone(), String::from_utf8_lossy(&self.crypto.open(&entry.ciphertext)?).into_owned());
                    out.vars += 1;
                }
                env_kind::SECRET => {
                    let value = String::from_utf8_lossy(&self.crypto.open(&entry.ciphertext)?).into_owned();
                    if !value.is_empty() {
                        out.secret_values.push(value.clone());
                    }
                    out.env.insert(entry.key.clone(), value);
                    out.secrets += 1;
                }
                env_kind::FILE => {
                    if let Some(t) = entry.target {
                        out.file_targets.push(t);
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// The absolute paths a project's `file` entries would materialize to under
    /// `worktree_path`, whether or not they currently exist. Used by teardown to know
    /// what to shred without depending on an in-memory materialize result (so it is
    /// robust across a daemon restart).
    pub fn materialized_file_paths(
        &self,
        store: &Store,
        project_id: &str,
        worktree_path: &Path,
    ) -> Result<Vec<PathBuf>, EnvError> {
        let mut paths = Vec::new();
        for entry in store.list_env_entries(project_id)? {
            if entry.kind == env_kind::FILE {
                if let Some(target) = entry.target.as_deref().filter(|t| !t.is_empty()) {
                    if validate_target(target).is_ok() {
                        paths.push(worktree_path.join(target));
                    }
                }
            }
        }
        Ok(paths)
    }

    /// `env.cleanup`: shred every materialized secret file for `project_id` under
    /// `worktree_path` before the worktree re-enters the pool (`environments.md` / On
    /// return). Each existing file is overwritten with zeros, flushed, and deleted.
    /// Returns how many files were shredded.
    ///
    /// Honest limitation: on a copy-on-write filesystem or an SSD with wear-leveling,
    /// overwrite-in-place does not guarantee the old bytes are physically gone. The
    /// overwrite defeats casual recovery from the reused worktree; the durable control
    /// remains that secrets live encrypted in the vault, not on disk.
    pub fn cleanup(
        &self,
        store: &Store,
        project_id: &str,
        worktree_path: &Path,
    ) -> Result<usize, EnvError> {
        let mut shredded = 0usize;
        for path in self.materialized_file_paths(store, project_id, worktree_path)? {
            if shred_file(&path)? {
                shredded += 1;
            }
        }
        Ok(shredded)
    }

    /// Diff each materialized `file` entry against its vault value at worktree return
    /// (`environments.md` / Drift guard). For every `file` entry whose on-disk content
    /// differs from the sealed vault value, produce a **value-masked** [`DriftEntry`]
    /// (key names only). Must run BEFORE `cleanup` shreds the files. A missing on-disk
    /// file is not drift (nothing was materialized or it was already cleaned). An entry
    /// sealed by another credential store is skipped (its value cannot be opened here).
    pub fn detect_drift(
        &self,
        store: &Store,
        project_id: &str,
        worktree_path: &Path,
    ) -> Result<Vec<DriftEntry>, EnvError> {
        let mut drifts = Vec::new();
        for entry in store.env_entries_sealed(project_id)? {
            if entry.kind != env_kind::FILE || entry.key_id != self.crypto.key_id() {
                continue;
            }
            let target = match entry.target.as_deref().filter(|t| !t.is_empty()) {
                Some(t) => t,
                None => continue,
            };
            if validate_target(target).is_err() {
                continue;
            }
            let abs = worktree_path.join(target);
            let on_disk = match fs::read_to_string(&abs) {
                Ok(text) => text,
                Err(_) => continue, // not materialized / already gone: not drift
            };
            let vault_value =
                String::from_utf8_lossy(&self.crypto.open(&entry.ciphertext)?).into_owned();
            if on_disk == vault_value {
                continue;
            }
            drifts.push(diff_masked(target, &vault_value, &on_disk));
        }
        Ok(drifts)
    }

    /// `env.import`: parse a `.env` file at `path`, classify each key, and ingest it.
    /// Existing keys are overwritten (an import is a refresh). Reports what it did.
    pub fn import(
        &self,
        store: &Store,
        project_id: &str,
        path: &Path,
    ) -> Result<ImportReport, EnvError> {
        let text = fs::read_to_string(path)
            .map_err(|e| EnvError::Invalid(format!("reading {}: {e}", path.display())))?;
        let mut report = ImportReport::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match parse_dotenv_line(line) {
                Some((key, value)) => {
                    let kind = classify_key(&key);
                    self.set_entry(store, project_id, &key, kind, &value, None)?;
                    if kind == env_kind::SECRET {
                        report.secrets += 1;
                    } else {
                        report.vars += 1;
                    }
                    report.entries.push((key, kind.to_string()));
                }
                None => report.skipped.push(raw.to_string()),
            }
        }
        Ok(report)
    }
}

/// Classify a `.env` key as `secret` or `var` by a name heuristic
/// (`environments.md` / Import assist: "parses, classifies, and ingests"). A key whose
/// uppercased name contains any credential-ish token is treated as a secret; everything
/// else is a plain var. Conservative on the safe side: a misclassified var is only ever
/// *over*-protected, never under-protected.
pub fn classify_key(key: &str) -> &'static str {
    const SECRET_MARKERS: &[&str] = &[
        "SECRET", "TOKEN", "PASSWORD", "PASSWD", "PWD", "APIKEY", "API_KEY", "ACCESS_KEY",
        "PRIVATE", "CREDENTIAL", "AUTH", "SIGNING", "CLIENT_SECRET", "SESSION_KEY", "ENCRYPTION",
    ];
    let upper = key.to_ascii_uppercase();
    if SECRET_MARKERS.iter().any(|m| upper.contains(m)) {
        env_kind::SECRET
    } else {
        env_kind::VAR
    }
}

/// Parse one non-blank, non-comment `.env` line into `(key, value)`, tolerating a
/// leading `export`, surrounding quotes on the value, and `=` inside the value.
/// Returns `None` when the line has no `=` or an empty key.
fn parse_dotenv_line(line: &str) -> Option<(String, String)> {
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let (key, value) = line.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
        return None;
    }
    let value = value.trim();
    let value = strip_quotes(value);
    Some((key.to_string(), value.to_string()))
}

/// Build a value-masked drift summary between the vault value and the on-disk content
/// (`environments.md` / Drift guard; `security.md`: diffs shown with values masked).
///
/// When both sides parse as `KEY=VALUE`, the summary is key-level: keys added on disk,
/// keys removed from disk, and keys whose value changed - names only, never values. When
/// either side is not env-parseable (a service-account json, an arbitrary file), the
/// summary is an opaque byte-level change with no content.
fn diff_masked(target: &str, vault_value: &str, on_disk: &str) -> DriftEntry {
    let vault_keys = parse_env_keys(vault_value);
    let disk_keys = parse_env_keys(on_disk);
    match (vault_keys, disk_keys) {
        (Some(vault), Some(disk)) => {
            let mut added_keys: Vec<String> =
                disk.iter().filter(|(k, _)| !vault.contains_key(*k)).map(|(k, _)| k.clone()).collect();
            let mut removed_keys: Vec<String> =
                vault.iter().filter(|(k, _)| !disk.contains_key(*k)).map(|(k, _)| k.clone()).collect();
            let mut changed_keys: Vec<String> = disk
                .iter()
                .filter(|(k, v)| vault.get(*k).is_some_and(|old| old != *v))
                .map(|(k, _)| k.clone())
                .collect();
            added_keys.sort();
            removed_keys.sort();
            changed_keys.sort();
            DriftEntry { target: target.to_string(), added_keys, removed_keys, changed_keys, opaque_change: false }
        }
        _ => DriftEntry {
            target: target.to_string(),
            added_keys: Vec::new(),
            removed_keys: Vec::new(),
            changed_keys: Vec::new(),
            opaque_change: true,
        },
    }
}

/// Parse a file's content into a `KEY -> VALUE` map if it looks like a `.env` file
/// (every non-blank, non-comment line is `KEY=VALUE`), else `None`.
fn parse_env_keys(text: &str) -> Option<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let mut saw_any = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = parse_dotenv_line(line)?;
        map.insert(key, value);
        saw_any = true;
    }
    if saw_any {
        Some(map)
    } else {
        None
    }
}

/// Strip a single matching pair of surrounding single or double quotes.
fn strip_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// Reject a file target that would escape the worktree or is absolute
/// (`security.md`: a materialized file must land inside the leased worktree). A vault
/// entry is user-controlled data, so this is validated on set and again on
/// materialize.
fn validate_target(target: &str) -> Result<(), EnvError> {
    let path = Path::new(target);
    if path.is_absolute() {
        return Err(EnvError::Invalid(format!("file target must be relative: {target}")));
    }
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                return Err(EnvError::Invalid(format!("file target must not contain '..': {target}")))
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(EnvError::Invalid(format!("file target must be a plain relative path: {target}")))
            }
            _ => {}
        }
    }
    Ok(())
}

/// Overwrite `path` with zeros, flush to disk, then delete it. Returns whether a file
/// was present. A missing file is not an error (idempotent teardown).
fn shred_file(path: &Path) -> Result<bool, EnvError> {
    let len = match fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => return Ok(false), // a dir at the target: not ours to shred
        Err(_) => return Ok(false), // already gone
    };
    // Overwrite in place before unlinking so the bytes are not left in the reused
    // worktree's slack (best-effort; see cleanup's honest limitation note).
    if let Ok(mut file) = fs::OpenOptions::new().write(true).open(path) {
        let zeros = vec![0u8; 8192];
        let mut remaining = len as usize;
        while remaining > 0 {
            let chunk = remaining.min(zeros.len());
            file.write_all(&zeros[..chunk])?;
            remaining -= chunk;
        }
        let _ = file.flush();
        let _ = file.sync_all();
    }
    fs::remove_file(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests;
