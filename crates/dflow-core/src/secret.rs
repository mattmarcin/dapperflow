//! Secret scrubbing v1 (`security.md` / Secret handling policy).
//!
//! Vault secrets follow one lifecycle: vault (encrypted at rest) -> materialization
//! (worktree env/file) -> use -> shredding at return. The boundaries *beyond* the
//! vault API are where a materialized value can leak into something durable, so this
//! module provides a value-matching scrubber and a registry of the plaintext secret
//! values currently materialized into each session.
//!
//! The scrubber is applied at exactly the boundaries `security.md` names:
//!
//! - **Scrollback / VT captures that leave the session**: the `session.peek` verb
//!   scrubs the captured screen text against the peeked session's secrets before it
//!   crosses the wire. The live screen a human is attached to is deliberately *not*
//!   scrubbed (they may legitimately need to see what happened).
//! - **Event payloads and timelines**: the store scrubs every `card_events` payload
//!   against the union of all live secret values, as defense in depth so a producer
//!   that forgot the rule never persists a raw value.
//!
//! # Known limitation, stated honestly (`security.md`)
//!
//! Value matching cannot catch a *transformed* secret: a value the agent base64-encodes,
//! splits across lines, or otherwise mutates before it reaches scrollback will not match
//! and will not be redacted. The scrubber is mitigation, not proof; the primary control
//! remains scoping which cards receive which vault entries. This is not a gap to be
//! "fixed" by adding more patterns - it is inherent to value matching.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The placeholder a matched secret value is replaced with (`security.md`).
pub const REDACTED: &str = "[dflow:redacted]";

/// The daemon-wide secret registry.
///
/// A single daemon process has exactly one live set of materialized secrets, so the
/// registry is a process singleton: the dispatch path registers a session's secrets
/// into it, `session.peek` and the event-payload scrubber read from it, and teardown
/// unregisters. Modelling it as one instance is what lets the store's `append_event`
/// apply defense-in-depth scrubbing with no per-call-site plumbing. The [`SecretRegistry`]
/// type itself stays independently constructable so its logic is unit-testable in
/// isolation; only this daemon-wide instance is shared.
pub fn registry() -> &'static SecretRegistry {
    static REGISTRY: OnceLock<SecretRegistry> = OnceLock::new();
    REGISTRY.get_or_init(SecretRegistry::default)
}

/// Minimum length a secret value must have to be scrubbed. Very short values (a
/// one- or two-character "secret") would match constantly and turn scrollback into
/// noise for no security benefit, so they are left alone; a real credential is long.
const MIN_SCRUBBABLE_LEN: usize = 4;

/// Replace every occurrence of each value in `secrets` with [`REDACTED`].
///
/// Values shorter than [`MIN_SCRUBBABLE_LEN`] are skipped (see the constant). Longer
/// values are replaced first so a secret that contains another as a substring redacts
/// wholesale rather than leaving a fragment.
pub fn scrub(text: &str, secrets: &[String]) -> String {
    let mut values: Vec<&String> = secrets.iter().filter(|s| s.len() >= MIN_SCRUBBABLE_LEN).collect();
    if values.is_empty() {
        return text.to_string();
    }
    values.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_string();
    for value in values {
        if out.contains(value.as_str()) {
            out = out.replace(value.as_str(), REDACTED);
        }
    }
    out
}

/// Replace every occurrence of each secret's bytes with [`REDACTED`] in a raw byte
/// stream, preserving surrounding non-UTF-8 bytes (ANSI escapes in a scrollback ring).
///
/// This is the byte-level twin of [`scrub`], used before scrollback persists to disk
/// (`security.md`: known secret values are pattern-replaced before scrollback persists).
/// Longer secrets are applied first so a substring secret never leaves a fragment.
pub fn scrub_bytes(bytes: &[u8], secrets: &[String]) -> Vec<u8> {
    let mut needles: Vec<&[u8]> =
        secrets.iter().filter(|s| s.len() >= MIN_SCRUBBABLE_LEN).map(|s| s.as_bytes()).collect();
    if needles.is_empty() {
        return bytes.to_vec();
    }
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut out = bytes.to_vec();
    for needle in needles {
        out = replace_bytes(&out, needle, REDACTED.as_bytes());
    }
    out
}

/// Replace all non-overlapping occurrences of `needle` in `haystack` with `to`.
fn replace_bytes(haystack: &[u8], needle: &[u8], to: &[u8]) -> Vec<u8> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if i + needle.len() <= haystack.len() && &haystack[i..i + needle.len()] == needle {
            out.extend_from_slice(to);
            i += needle.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// Whether `text` contains any (scrubbable) secret value verbatim.
pub fn contains_secret(text: &str, secrets: &[String]) -> bool {
    secrets
        .iter()
        .filter(|s| s.len() >= MIN_SCRUBBABLE_LEN)
        .any(|s| text.contains(s.as_str()))
}

/// Recursively scrub every string in a JSON value against `secrets` (event-payload
/// defense in depth: `security.md` / Event payloads and timelines).
pub fn scrub_json(value: &mut serde_json::Value, secrets: &[String]) {
    match value {
        serde_json::Value::String(s) => {
            if contains_secret(s, secrets) {
                *s = scrub(s, secrets);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                scrub_json(item, secrets);
            }
        }
        serde_json::Value::Object(map) => {
            for (_k, v) in map.iter_mut() {
                scrub_json(v, secrets);
            }
        }
        _ => {}
    }
}

/// A live registry of the plaintext secret values materialized into each scope.
///
/// A "scope" is an opaque key the daemon chooses: dispatch registers a session's
/// secrets under the session id (so `session.peek` scrubs exactly that session's
/// values); the diagnostic `env.materialize` verb registers under a worktree id.
/// Cleanup unregisters the scope at teardown. The registry is in-memory: a daemon
/// restart interrupts every session, so no scope needs to outlive the run, and the
/// materialized files are re-derivable from the vault for shredding regardless.
#[derive(Default)]
pub struct SecretRegistry {
    by_scope: Mutex<HashMap<String, Vec<String>>>,
}

impl SecretRegistry {
    /// Register (replacing) the secret values live for `scope`. Empty values are
    /// dropped; an empty set removes the scope entirely.
    pub fn register(&self, scope: &str, values: Vec<String>) {
        let values: Vec<String> = values.into_iter().filter(|v| !v.is_empty()).collect();
        let mut map = self.by_scope.lock().expect("secret registry poisoned");
        if values.is_empty() {
            map.remove(scope);
        } else {
            map.insert(scope.to_string(), values);
        }
    }

    /// Drop a scope's secrets (teardown/cleanup).
    pub fn unregister(&self, scope: &str) {
        self.by_scope.lock().expect("secret registry poisoned").remove(scope);
    }

    /// The secret values registered for one scope (for `session.peek`).
    pub fn values_for(&self, scope: &str) -> Vec<String> {
        self.by_scope.lock().expect("secret registry poisoned").get(scope).cloned().unwrap_or_default()
    }

    /// The union of every live secret value (for event-payload defense in depth).
    pub fn all_values(&self) -> Vec<String> {
        let map = self.by_scope.lock().expect("secret registry poisoned");
        let mut all: Vec<String> = Vec::new();
        for values in map.values() {
            for v in values {
                if !all.contains(v) {
                    all.push(v.clone());
                }
            }
        }
        all
    }

    /// Whether any scope holds secrets (fast path so scrubbing is a no-op with an
    /// empty vault).
    pub fn is_empty(&self) -> bool {
        self.by_scope.lock().expect("secret registry poisoned").is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_replaces_all_occurrences() {
        let secrets = vec!["sk-supersecret".to_string()];
        let text = "using sk-supersecret and again sk-supersecret here";
        let out = scrub(text, &secrets);
        assert!(!out.contains("sk-supersecret"));
        assert_eq!(out.matches(REDACTED).count(), 2);
    }

    #[test]
    fn scrub_skips_trivially_short_values() {
        // A two-char "secret" would redact half of everything; it is left alone.
        let secrets = vec!["ab".to_string()];
        let text = "about a cab in a lab";
        assert_eq!(scrub(text, &secrets), text);
    }

    #[test]
    fn overlapping_values_redact_wholesale() {
        // The longer value is applied first, so the short one never leaves a fragment.
        let secrets = vec!["secret".to_string(), "supersecret-token".to_string()];
        let out = scrub("value=supersecret-token end", &secrets);
        assert_eq!(out, format!("value={REDACTED} end"));
    }

    #[test]
    fn transformed_secret_is_not_caught_documented_limitation() {
        // Honest limitation: a base64-transformed secret does not match the raw value.
        let raw = "sk-supersecret".to_string();
        let transformed = "c2stc3VwZXJzZWNyZXQ="; // base64("sk-supersecret")
        let out = scrub(transformed, &[raw]);
        assert_eq!(out, transformed, "value matching cannot catch a transformed secret");
    }

    #[test]
    fn scrub_json_walks_nested_strings() {
        let secrets = vec!["hunter2password".to_string()];
        let mut payload = serde_json::json!({
            "note": "logging in with hunter2password now",
            "nested": { "cmd": "export PW=hunter2password" },
            "list": ["hunter2password", "safe"],
            "count": 3,
        });
        scrub_json(&mut payload, &secrets);
        let s = payload.to_string();
        assert!(!s.contains("hunter2password"));
        assert_eq!(payload["count"], 3, "non-strings are untouched");
    }

    #[test]
    fn registry_scopes_and_union() {
        let reg = SecretRegistry::default();
        assert!(reg.is_empty());
        reg.register("session-a", vec!["aaaa-secret".into(), "".into()]);
        reg.register("session-b", vec!["bbbb-secret".into()]);
        assert_eq!(reg.values_for("session-a"), vec!["aaaa-secret".to_string()]);
        let all = reg.all_values();
        assert!(all.contains(&"aaaa-secret".to_string()));
        assert!(all.contains(&"bbbb-secret".to_string()));
        reg.unregister("session-a");
        assert!(reg.values_for("session-a").is_empty());
        // An empty registration removes the scope.
        reg.register("session-b", vec![]);
        assert!(reg.is_empty());
    }
}
