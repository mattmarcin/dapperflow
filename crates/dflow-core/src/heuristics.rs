//! Tier-3 screen heuristics (`adapters.md` / three-tier signal model).
//!
//! Two jobs: (1) a coarse lifecycle read - is the pane busy working, or parked on a
//! prompt awaiting the user - used to raise Needs You and to cross-check tier-2; and
//! (2) composer-state classification (empty / typed / popup-open) with ghost-text
//! stripping, the input verified submit reads before and after it types
//! (`adapters.md` / Verified submit). Manifest data (`manifest.rs`) drives the busy
//! signature, the ghost-text styles, and the trust-dialog pattern, so hardening a
//! harness is a manifest edit, not a code change.

use dflow_proto::StyledSnapshot;

use crate::manifest::{bundled_manifests, Manifest};

/// High-precision permission/trust prompt phrases (lowercased match). These are the
/// tier-3 fallback; the manifest trust-dialog pattern is checked alongside them.
const INPUT_PROMPTS: &[&str] = &[
    "do you want to proceed",
    "do you want to make this edit",
    "do you want to create",
    "do you want to run",
    "do you trust the files",
    "do you trust the authors",
    // [L] VERIFIED-LOCAL 2026-07-04: the real claude first-run trust dialog text.
    "trust this folder",
    "one you trust",
    "press enter to continue",
];

/// The busy-working signature for `harness`, from its manifest (`adapters.md` /
/// signals.busy_signature), falling back to the claude seed when unmanifested.
fn busy_signature(harness: &str) -> String {
    bundled_manifests()
        .get(harness)
        .map(|m| m.signals.busy_signature.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "esc to interrupt".to_string())
}

/// Whether the pane is busy working, by the harness busy signature (tier 3).
pub fn is_busy(harness: &str, screen: &str) -> bool {
    let sig = busy_signature(harness);
    !sig.is_empty() && screen.to_lowercase().contains(&sig.to_lowercase())
}

/// Whether the pane is a trust/permission dialog (`adapters.md` / dialogs). Checked
/// against the manifest trust pattern plus the high-precision phrase list.
pub fn is_trust_dialog(harness: &str, screen: &str) -> bool {
    let hay = screen.to_lowercase();
    if let Some(rule) = bundled_manifests().get(harness).and_then(|m| m.dialogs.trust.as_ref()) {
        let pat = rule.pattern.to_lowercase();
        if !pat.is_empty() && hay.contains(&pat) && hay.contains("trust") {
            return true;
        }
    }
    hay.contains("do you trust the files")
        || hay.contains("do you trust the authors")
        || hay.contains("trust this folder")
}

/// Whether the pane is waiting on the user (an idle composer at a permission or trust
/// prompt). Never true while the harness is busy working.
pub fn needs_input(harness: &str, screen: &str) -> bool {
    if is_busy(harness, screen) {
        return false;
    }
    let hay = screen.to_lowercase();
    if INPUT_PROMPTS.iter().any(|p| hay.contains(p)) {
        return true;
    }
    is_trust_dialog(harness, screen)
}

/// Composer classification for verified submit (`adapters.md` / Verified submit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerState {
    /// No typed text (blank, or only a prompt marker and ghost/placeholder text).
    Empty,
    /// Real user text is present in the composer.
    HasText,
    /// A completion popup is open over/near the composer (can swallow the first Enter).
    PopupOpen,
}

/// Common composer prompt markers stripped before reading typed text.
const PROMPT_MARKERS: &[&str] = &["\u{276f}", "\u{00bb}", "\u{2502}", "\u{258c}", ">", "|", "#", "$"];

/// Whether a styled run is ghost/placeholder text per the manifest ghost styles.
fn is_ghost(run: &dflow_proto::StyledRun, ghost_styles: &[String]) -> bool {
    ghost_styles.iter().any(|style| match style.as_str() {
        "dim" | "faint" => run.dim,
        "italic" => run.italic,
        "underline" => run.underline,
        _ => false,
    })
}

/// Strip a leading prompt marker and surrounding whitespace from a composer line.
fn strip_prompt_marker(s: &str) -> String {
    let mut t = s.trim_start();
    for marker in PROMPT_MARKERS {
        if let Some(rest) = t.strip_prefix(*marker) {
            t = rest.trim_start();
            break;
        }
    }
    t.trim().to_string()
}

/// The real (non-ghost) typed text on the composer row `row` of `snapshot`, ghost text
/// stripped per `ghost_styles` and the prompt marker removed (`adapters.md` /
/// Verified submit, step 1: classify, stripping ghost-text styles per manifest).
pub fn composer_text(snapshot: &StyledSnapshot, row: u16, ghost_styles: &[String]) -> String {
    let runs = match snapshot.lines.get(row as usize) {
        Some(r) => r,
        None => return String::new(),
    };
    let mut s = String::new();
    for run in runs {
        if is_ghost(run, ghost_styles) {
            continue;
        }
        s.push_str(&run.text);
    }
    strip_prompt_marker(&s)
}

/// Whether a completion popup is open near the composer row. Heuristic (v0, refined by
/// live probes): a selected menu entry renders inverse, so a non-composer line within a
/// short window of the composer carrying a non-blank inverse run signals an open popup.
fn popup_open(snapshot: &StyledSnapshot, composer_row: u16) -> bool {
    let composer = composer_row as i32;
    for (idx, runs) in snapshot.lines.iter().enumerate() {
        let dist = (idx as i32 - composer).abs();
        if idx as i32 == composer || dist > 8 {
            continue;
        }
        if runs.iter().any(|r| r.inverse && r.text.trim().chars().any(|c| !c.is_whitespace())) {
            return true;
        }
    }
    false
}

/// Classify the composer state on `snapshot` at cursor row `row`, using the harness
/// manifest for ghost-text styles (`adapters.md` / Verified submit, step 1).
pub fn classify_composer(snapshot: &StyledSnapshot, row: u16, manifest: &Manifest) -> ComposerState {
    if popup_open(snapshot, row) {
        return ComposerState::PopupOpen;
    }
    let text = composer_text(snapshot, row, &manifest.composer.ghost_text_styles);
    if text.is_empty() {
        ComposerState::Empty
    } else {
        ComposerState::HasText
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dflow_proto::StyledRun;

    fn run(text: &str) -> StyledRun {
        StyledRun {
            text: text.to_string(),
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }

    fn dim(text: &str) -> StyledRun {
        StyledRun { dim: true, ..run(text) }
    }

    fn inverse(text: &str) -> StyledRun {
        StyledRun { inverse: true, ..run(text) }
    }

    fn snap(lines: Vec<Vec<StyledRun>>) -> StyledSnapshot {
        StyledSnapshot { cols: 80, rows: lines.len() as u16, lines }
    }

    #[test]
    fn permission_prompt_is_needs_input() {
        let screen = "\
Claude wants to edit src/main.rs

Do you want to proceed?
  1. Yes
  2. No, tell Claude what to do differently";
        assert!(needs_input("claude", screen));
    }

    #[test]
    fn trust_prompt_is_needs_input_and_trust_dialog() {
        let screen = "Do you trust the files in this folder?";
        assert!(needs_input("claude", screen));
        assert!(is_trust_dialog("claude", screen));
    }

    #[test]
    fn busy_pane_is_not_needs_input() {
        assert!(is_busy("claude", "Booting... (esc to interrupt)"));
        assert!(!needs_input("claude", "Do you want to proceed... working (esc to interrupt)"));
    }

    #[test]
    fn ordinary_output_is_not_needs_input() {
        assert!(!needs_input("claude", "Compiling dflow-core\n   Finished in 2.5s"));
        assert!(!needs_input("claude", ""));
    }

    #[test]
    fn busy_signature_comes_from_manifest() {
        // opencode's manifest busy signature is also "esc to interrupt" (seed); an
        // unmanifested harness falls back to the same seed.
        assert!(is_busy("opencode", "thinking (esc to interrupt)"));
        assert!(is_busy("unmanifested", "please wait (esc to interrupt)"));
    }

    #[test]
    fn composer_text_strips_ghost_and_prompt_marker() {
        // "> hello" typed, with dim ghost placeholder appended.
        let line = vec![run("> hello"), dim("  type a message...")];
        let text = composer_text(&snap(vec![line]), 0, &["dim".to_string()]);
        assert_eq!(text, "hello");
    }

    #[test]
    fn empty_composer_is_empty_even_with_ghost() {
        let m = bundled_manifests().get("claude").unwrap();
        let line = vec![run("> "), dim("type a message...")];
        assert_eq!(classify_composer(&snap(vec![line]), 0, m), ComposerState::Empty);
    }

    #[test]
    fn typed_composer_is_has_text() {
        let m = bundled_manifests().get("claude").unwrap();
        let line = vec![run("> fix the bug")];
        assert_eq!(classify_composer(&snap(vec![line]), 0, m), ComposerState::HasText);
    }

    #[test]
    fn inverse_menu_entry_is_popup_open() {
        let m = bundled_manifests().get("claude").unwrap();
        let lines = vec![
            vec![inverse("/commit  commit changes")],
            vec![run("/compact  compact context")],
            vec![run("> /co")],
        ];
        assert_eq!(classify_composer(&snap(lines), 2, m), ComposerState::PopupOpen);
    }
}
