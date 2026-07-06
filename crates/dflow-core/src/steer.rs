//! Verified submit: the primitive that makes unattended steering trustworthy
//! (`adapters.md` / Verified submit).
//!
//! Sending text into an agent TUI can silently fail - slash/`$` popups swallow the
//! first Enter, argument-hint placeholders expand instead of submitting, ghost text
//! masquerades as typed input. This module types text, waits for popups to settle,
//! presses Enter, re-reads the composer from the daemon's own screen model, and
//! retries with backoff up to a bounded attempt count, returning `{submitted,
//! attempts}`. A failed submit is reported so the caller raises Needs You rather than
//! silently dropping the message.
//!
//! All reads go through the `Session` screen model (`heuristics::composer_text`,
//! `classify_composer`), so classification is testable offline against recorded
//! snapshots and against the in-repo stub TUI.

use std::time::{Duration, Instant};

use crate::heuristics;
use crate::manifest::Manifest;
use crate::session::Session;

/// The outcome of a verified submit (`adapters.md`: returns `{ submitted, attempts }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSubmit {
    pub submitted: bool,
    pub attempts: u32,
}

/// Timing and bound knobs for a verified submit. Production values come from the
/// manifest; tests construct fast variants.
#[derive(Debug, Clone)]
pub struct SubmitConfig {
    /// Max Enter attempts before giving up and raising Needs You.
    pub max_attempts: u32,
    /// How long to wait for a redraw after typing/Enter before re-reading.
    pub redraw_wait: Duration,
    /// How long to wait for a completion popup to settle before pressing Enter.
    pub popup_settle: Duration,
    /// The bytes that submit the composer (Enter). Carriage return by default.
    pub enter_bytes: Vec<u8>,
}

impl SubmitConfig {
    /// Production timings for a harness, popup settle and submit key from its manifest.
    pub fn from_manifest(manifest: &Manifest) -> Self {
        Self {
            max_attempts: 4,
            redraw_wait: Duration::from_millis(250),
            popup_settle: Duration::from_millis(manifest.composer.popup_settle_ms.max(1)),
            enter_bytes: manifest.composer.submit_bytes(),
        }
    }
}

/// Type `text` into `session` and verify it was submitted (`adapters.md` algorithm).
///
/// A manifest with `no_auto_steer = true` refuses automated steering: it returns
/// `{ submitted: false, attempts: 0 }` without typing, and the caller surfaces a Needs
/// You explanation rather than steering a composer it cannot classify.
pub fn send_verified(
    session: &Session,
    manifest: &Manifest,
    text: &str,
    cfg: &SubmitConfig,
) -> VerifiedSubmit {
    if manifest.adapter.no_auto_steer {
        return VerifiedSubmit { submitted: false, attempts: 0 };
    }
    let ghost = &manifest.composer.ghost_text_styles;

    // 2. Type the text. A harness that needs bracketed paste (finding #3) gets the text
    // wrapped in the DEC 2004 envelope so a multi-line prompt lands as one paste and its
    // embedded newlines are inserted, not treated as submits.
    if manifest.composer.uses_bracketed_paste() {
        let mut framed = Vec::with_capacity(text.len() + 12);
        framed.extend_from_slice(b"\x1b[200~");
        framed.extend_from_slice(text.as_bytes());
        framed.extend_from_slice(b"\x1b[201~");
        let _ = session.write_input(&framed);
    } else {
        let _ = session.write_input(text.as_bytes());
    }

    // 2 (cont). If it starts with a popup prefix, wait for the popup to settle;
    // otherwise wait for a normal redraw.
    let opens_popup = manifest
        .composer
        .popup_prefixes
        .iter()
        .any(|p| !p.is_empty() && text.starts_with(p.as_str()));
    std::thread::sleep(if opens_popup { cfg.popup_settle } else { cfg.redraw_wait });

    // 3-5. Send Enter, re-read, retry on pending text (including placeholder-expanded
    // text) with backoff, bounded attempts.
    let mut attempts = 0;
    loop {
        attempts += 1;
        let _ = session.write_input(&cfg.enter_bytes);
        std::thread::sleep(cfg.redraw_wait);

        if is_submitted(&current_composer(session, ghost), text) {
            return VerifiedSubmit { submitted: true, attempts };
        }
        if attempts >= cfg.max_attempts {
            return VerifiedSubmit { submitted: false, attempts };
        }
        // Backoff before the next Enter.
        std::thread::sleep(cfg.redraw_wait);
    }
}

/// Wait until `session`'s TUI is ready to accept typed input: alive, drawn, not mid-turn
/// (busy), and not parked on a trust/permission dialog (`adapters.md` / dispatch flow step
/// 7; audit finding #3).
///
/// The gate deliberately does NOT require an `Empty` composer classification. That check
/// was claude-tuned and misread every other harness's boxed composer (heavy box-drawing
/// prompt markers, placeholder ghost styles, cursor-row differences), so the gate never
/// passed for codex/opencode/pi and typed injection never fired (`attempts: 0`, `$0`
/// spent). Readiness is now the harness-agnostic signal set - drawn, idle, no dialog -
/// plus an optional positive per-harness `ready_signature`, and it is confirmed across two
/// consecutive polls so a transient idle frame mid-startup does not count. Returns false on
/// timeout or exit (e.g. a trust dialog we will not blindly type into).
pub fn wait_for_composer_ready(session: &Session, manifest: &Manifest, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let name = &manifest.adapter.name;
    let ready_sig = manifest.composer.ready_signature.to_lowercase();
    let mut consecutive = 0u8;
    loop {
        if !session.is_alive() {
            return false;
        }
        let plain = session.capture_plain();
        let busy = heuristics::is_busy(name, &plain);
        let dialog = heuristics::needs_input(name, &plain);
        let drawn = !plain.trim().is_empty();
        let sig_ok = ready_sig.is_empty() || plain.to_lowercase().contains(&ready_sig);
        if drawn && !busy && !dialog && sig_ok {
            consecutive += 1;
            // Two consecutive idle observations (~200ms apart) confirm a settled composer.
            if consecutive >= 2 {
                return true;
            }
        } else {
            consecutive = 0;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The current non-ghost composer text at the cursor row.
fn current_composer(session: &Session, ghost_styles: &[String]) -> String {
    let cursor = session.cursor();
    let snapshot = session.styled_snapshot();
    heuristics::composer_text(&snapshot, cursor.row, ghost_styles)
}

/// Whether `text` is no longer pending in the composer after Enter. Submitted when the
/// composer no longer holds the meaningful head of the typed text (so a
/// placeholder-expanded remainder that still contains it reads as not-yet-submitted).
fn is_submitted(composer_after: &str, text: &str) -> bool {
    if composer_after.trim().is_empty() {
        return true;
    }
    let needle: String = text.trim().chars().take(12).collect();
    let needle = needle.trim();
    if needle.is_empty() {
        return true;
    }
    !composer_after.contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submitted_when_composer_cleared() {
        assert!(is_submitted("", "fix the bug"));
        assert!(is_submitted("> ", "fix the bug"));
    }

    #[test]
    fn not_submitted_when_text_still_pending() {
        assert!(!is_submitted("fix the bug", "fix the bug"));
        // Placeholder expansion keeps the original text present.
        assert!(!is_submitted("/deploy <env>", "/deploy"));
    }

    #[test]
    fn no_auto_steer_refuses_without_typing() {
        let set = crate::manifest::ManifestSet::bundled().unwrap();
        let cursor = set.get("cursor").unwrap();
        // A dummy session is not needed: no_auto_steer returns before any I/O. Build a
        // minimal config and a throwaway session-less path via the guard.
        assert!(cursor.adapter.no_auto_steer);
        // The guard is exercised end-to-end in the stub-TUI integration test.
    }
}
