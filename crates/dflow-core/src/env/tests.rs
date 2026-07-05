//! Vault round-trip, materialization, teardown-shred, and import-assist tests.
//!
//! These use a deterministic in-test crypto backend so the vault's logic is exercised
//! identically on every platform; the DPAPI backend itself is covered by
//! `env::crypto::tests` on Windows.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::store::env::env_kind;
use crate::store::Store;

/// A reversible, non-persisting-plaintext test backend (XOR pad). Real enough to prove
/// the store only ever holds ciphertext, without depending on the OS credential store.
struct FakeCrypto;

impl VaultCrypto for FakeCrypto {
    fn key_id(&self) -> &'static str {
        "dpapi" // masquerade as the default so materialize does not skip entries
    }
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Ok(plaintext.iter().map(|b| b ^ 0x33).collect())
    }
    fn open(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Ok(ciphertext.iter().map(|b| b ^ 0x33).collect())
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-vault-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn vault_and_project() -> (EnvVault, Store, String) {
    let vault = EnvVault::with_crypto(Box::new(FakeCrypto));
    let store = Store::open_in_memory().unwrap();
    let project = store.add_project("/tmp/proj", "proj", "main", "pr").unwrap();
    (vault, store, project.id)
}

#[test]
fn store_holds_only_ciphertext_never_plaintext() {
    let (vault, store, pid) = vault_and_project();
    vault.set_entry(&store, &pid, "API_KEY", env_kind::SECRET, "sk-plaintext-value", None).unwrap();
    let sealed = store.env_entries_sealed(&pid).unwrap();
    assert_eq!(sealed.len(), 1);
    assert_ne!(sealed[0].ciphertext, b"sk-plaintext-value", "the vault must never persist plaintext");
}

#[test]
fn set_materialize_spawn_env_and_file_then_return_shreds() {
    let (vault, store, pid) = vault_and_project();
    vault.set_entry(&store, &pid, "PUBLIC_URL", env_kind::VAR, "https://example.test", None).unwrap();
    vault.set_entry(&store, &pid, "DB_PASSWORD", env_kind::SECRET, "hunter2-super-secret", None).unwrap();
    vault
        .set_entry(&store, &pid, "dev-vars", env_kind::FILE, "SECRET_TOKEN=abc-file-secret\n", Some(".dev.vars"))
        .unwrap();

    let worktree = temp_dir("materialize");

    // Materialize: vars + secrets land in the spawn env; the file lands on disk.
    let mat = vault.materialize(&store, &pid, &worktree).unwrap();
    assert_eq!(mat.env.get("PUBLIC_URL").map(String::as_str), Some("https://example.test"));
    assert_eq!(mat.env.get("DB_PASSWORD").map(String::as_str), Some("hunter2-super-secret"));
    assert_eq!(mat.vars, 1);
    assert_eq!(mat.secrets, 1);

    let file_path = worktree.join(".dev.vars");
    assert!(file_path.exists(), "the file entry must be written into the worktree");
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "SECRET_TOKEN=abc-file-secret\n");

    // The secret value and the file contents are surfaced for the scrubber.
    assert!(mat.secret_values.iter().any(|v| v == "hunter2-super-secret"));
    assert!(mat.secret_values.iter().any(|v| v.contains("abc-file-secret")));

    // Return/teardown shreds the materialized secret file before the worktree is reused.
    let shredded = vault.cleanup(&store, &pid, &worktree).unwrap();
    assert_eq!(shredded, 1);
    assert!(!file_path.exists(), "the materialized secret file must be shredded on return");

    std::fs::remove_dir_all(&worktree).ok();
}

#[test]
fn list_never_leaks_values() {
    let (vault, store, pid) = vault_and_project();
    vault.set_entry(&store, &pid, "TOKEN", env_kind::SECRET, "top-secret-token", None).unwrap();
    vault.set_entry(&store, &pid, "MODE", env_kind::VAR, "production", None).unwrap();
    let metas = vault.list_entries(&store, &pid).unwrap();
    // The only fields present are names, kinds, targets, versions - never a value. A
    // metadata struct that carried a value would fail to compile against this assertion.
    assert_eq!(metas.len(), 2);
    let debug = format!("{metas:?}");
    assert!(!debug.contains("top-secret-token"), "env.list must never surface a value");
    assert!(!debug.contains("production"));
}

#[test]
fn file_target_escaping_worktree_is_rejected() {
    let (vault, store, pid) = vault_and_project();
    let err = vault
        .set_entry(&store, &pid, "escape", env_kind::FILE, "x", Some("../../etc/passwd"))
        .unwrap_err();
    assert!(matches!(err, EnvError::Invalid(_)), "a target with '..' must be rejected");
    let err = vault.set_entry(&store, &pid, "abs", env_kind::FILE, "x", Some("/etc/hosts")).unwrap_err();
    assert!(matches!(err, EnvError::Invalid(_)), "an absolute target must be rejected");
}

#[test]
fn import_classifies_secrets_by_name_heuristic() {
    let (vault, store, pid) = vault_and_project();
    let dir = temp_dir("import");
    let env_path = dir.join(".env");
    std::fs::write(
        &env_path,
        "# a comment\n\
         export PUBLIC_HOST=example.com\n\
         DATABASE_URL=\"postgres://localhost/app\"\n\
         API_KEY=sk-abc123\n\
         STRIPE_SECRET_KEY='sk_live_xyz'\n\
         SESSION_PASSWORD=p4ss\n\
         not a valid line\n\
         PORT=3000\n",
    )
    .unwrap();

    let report = vault.import(&store, &pid, &env_path).unwrap();

    // Secret-looking keys are classified secret; the rest are vars.
    let kind_of = |k: &str| report.entries.iter().find(|(key, _)| key == k).map(|(_, kind)| kind.clone());
    assert_eq!(kind_of("API_KEY").as_deref(), Some(env_kind::SECRET));
    assert_eq!(kind_of("STRIPE_SECRET_KEY").as_deref(), Some(env_kind::SECRET));
    assert_eq!(kind_of("SESSION_PASSWORD").as_deref(), Some(env_kind::SECRET));
    assert_eq!(kind_of("PUBLIC_HOST").as_deref(), Some(env_kind::VAR));
    assert_eq!(kind_of("DATABASE_URL").as_deref(), Some(env_kind::VAR));
    assert_eq!(kind_of("PORT").as_deref(), Some(env_kind::VAR));

    assert_eq!(report.secrets, 3);
    assert_eq!(report.vars, 3);
    assert_eq!(report.imported(), 6);
    // The unparseable line is reported, not silently dropped.
    assert_eq!(report.skipped, vec!["not a valid line".to_string()]);

    // Quotes were stripped on the way in: materializing recovers the unquoted value.
    let worktree = temp_dir("import-mat");
    let mat = vault.materialize(&store, &pid, &worktree).unwrap();
    assert_eq!(mat.env.get("DATABASE_URL").map(String::as_str), Some("postgres://localhost/app"));
    assert_eq!(mat.env.get("STRIPE_SECRET_KEY").map(String::as_str), Some("sk_live_xyz"));

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&worktree).ok();
}

#[test]
fn detect_drift_yields_a_value_masked_key_diff() {
    let (vault, store, pid) = vault_and_project();
    vault
        .set_entry(
            &store,
            &pid,
            "dev-vars",
            env_kind::FILE,
            "API_KEY=old-secret\nMODE=dev\nREGION=us\n",
            Some(".dev.vars"),
        )
        .unwrap();
    let worktree = temp_dir("drift");
    vault.materialize(&store, &pid, &worktree).unwrap();

    // No edit yet: no drift.
    assert!(vault.detect_drift(&store, &pid, &worktree).unwrap().is_empty());

    // The agent edits the materialized file: adds a key, drops one, changes one.
    let file = worktree.join(".dev.vars");
    std::fs::write(&file, "API_KEY=NEW-secret-value\nREGION=us\nNEW_FLAG=on\n").unwrap();

    let drifts = vault.detect_drift(&store, &pid, &worktree).unwrap();
    assert_eq!(drifts.len(), 1);
    let d = &drifts[0];
    assert_eq!(d.target, ".dev.vars");
    assert_eq!(d.added_keys, vec!["NEW_FLAG".to_string()]);
    assert_eq!(d.removed_keys, vec!["MODE".to_string()]);
    assert_eq!(d.changed_keys, vec!["API_KEY".to_string()]);
    assert!(!d.opaque_change);

    // The masked summary carries key NAMES only - never the changed values.
    let debug = format!("{d:?}");
    assert!(!debug.contains("NEW-secret-value"), "drift summary must mask values");
    assert!(!debug.contains("old-secret"), "drift summary must mask values");

    std::fs::remove_dir_all(&worktree).ok();
}

#[test]
fn cleanup_is_idempotent_when_nothing_materialized() {
    let (vault, store, pid) = vault_and_project();
    vault.set_entry(&store, &pid, "VAR", env_kind::VAR, "v", None).unwrap();
    let worktree = temp_dir("noclean");
    // No file entries and nothing on disk: cleanup shreds nothing and does not error.
    assert_eq!(vault.cleanup(&store, &pid, &worktree).unwrap(), 0);
    std::fs::remove_dir_all(&worktree).ok();
}
