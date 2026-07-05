//! Harness launch resolution (`adapters.md` / capability matrix, Adapter manifests).
//!
//! Phase 2 makes adapter manifests (`manifest.rs`) the source of truth: the launch
//! line, autonomy/model/effort flags, and resume argv are manifest data, not a
//! built-in table. This module assembles concrete argv from a manifest plus the
//! launcher command, brief, and axes, and keeps the `DFLOW_LAUNCH_<NAME>` override as
//! the seam tests use to substitute a stub without a real CLI:
//!
//! ```text
//! DFLOW_LAUNCH_<HARNESS> = ["prog","arg","{brief}"]   (JSON array), or
//! DFLOW_LAUNCH_<HARNESS> = prog arg {brief}           (whitespace argv)
//! ```
//!
//! `{brief}` is replaced with the composed brief; omit it to launch without one.

use crate::manifest::bundled_manifests;

/// The default harness when a dispatch request names none.
pub const DEFAULT_HARNESS: &str = "claude";

/// Adapter families this build knows, plus `custom` for user launchers whose
/// behavior is not one of the manifests (`data-model.md` / agents.adapter). Kept in
/// one place so validation and launch stay in sync; a test asserts it matches the
/// bundled manifest set.
pub const KNOWN_ADAPTERS: &[&str] = &["claude", "codex", "opencode", "cursor", "pi", "custom"];

/// Built-in harnesses the legacy (non-launcher) dispatch path will launch directly.
/// Other families (`cursor`, `pi`) are reachable only through a configured launcher.
const BUILTIN_DISPATCHABLE: &[&str] = &["claude", "codex", "opencode"];

/// Whether `adapter` is a known behavior family (has a manifest) or `custom`.
pub fn is_known_adapter(adapter: &str) -> bool {
    adapter == "custom" || bundled_manifests().get(adapter).is_some()
}

/// Resolve the launch argv for a legacy built-in `harness`, substituting `brief` and
/// the optional model/effort axes from the adapter manifest.
///
/// Taken when no configured launcher matches. Covers only the built-in dispatchable
/// families (claude/codex/opencode); other families need a configured launcher. A
/// `DFLOW_LAUNCH_<H>` override still fully replaces the argv (the test seam). Returns
/// `None` for an unknown/undispatchable harness with no override.
pub fn harness_command(
    harness: &str,
    brief: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Option<Vec<String>> {
    if let Some(argv) = launch_override(harness, brief) {
        return Some(argv);
    }
    if !BUILTIN_DISPATCHABLE.contains(&harness) {
        return None;
    }
    let manifest = bundled_manifests().get(harness)?;
    Some(manifest.build_launch(harness, Some(brief), model, effort))
}

/// Resolve the launch argv for a configured launcher (`product.md` / Settings >
/// Agents): the manifest launch line built with the launcher's `command`, the brief,
/// and the model/effort axes, then the launcher's `extra_args` last (so they never
/// split a flag from its value).
///
/// A `DFLOW_LAUNCH_<NAME>` override (keyed by the launcher `name`) replaces the
/// manifest base while still honoring `extra_args`; it is the seam tests use to
/// substitute a stub executable without a real agent CLI.
pub fn launcher_command(
    name: &str,
    adapter: &str,
    command: &str,
    brief: &str,
    extra_args: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let mut argv = match launch_override(name, brief) {
        Some(base) => base,
        None => manifest_launch(adapter, command, Some(brief), model, effort),
    };
    argv.extend(extra_args.iter().cloned());
    argv
}

/// Resolve the launch argv for a configured launcher started as a bare interactive
/// terminal (no brief): the manifest launch line with no `{prompt}`, then `extra_args`.
/// Used by `session.create` with an `agent`.
pub fn launcher_interactive_command(
    adapter: &str,
    command: &str,
    extra_args: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let mut argv = manifest_launch(adapter, command, None, model, effort);
    argv.extend(extra_args.iter().cloned());
    argv
}

/// Resolve the resume argv for a harness family from its manifest, or `None` when the
/// manifest declares no resume mechanism (`adapters.md` / controls.resume).
pub fn resume_command(adapter: &str, command: &str, resume_ref: &str) -> Option<Vec<String>> {
    bundled_manifests().get(adapter)?.build_resume(command, resume_ref)
}

/// Build a launch from the adapter manifest, falling back to a bare positional launch
/// for `custom` (or any unmanifested) adapter: `[command] (+ prompt)`.
fn manifest_launch(
    adapter: &str,
    command: &str,
    prompt: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    match bundled_manifests().get(adapter) {
        Some(manifest) => manifest.build_launch(command, prompt, model, effort),
        None => {
            let mut argv = vec![command.to_string()];
            if let Some(p) = prompt {
                argv.push(p.to_string());
            }
            argv
        }
    }
}

/// Compose the v0 dispatch brief: the card title, then the card brief if present.
pub fn compose_brief(title: &str, brief: Option<&str>) -> String {
    match brief {
        Some(b) if !b.trim().is_empty() => format!("{}\n\n{}", title.trim(), b.trim()),
        _ => title.trim().to_string(),
    }
}

/// A single-line preview of `text`, truncated to `max` characters (session list).
pub fn preview(text: &str, max: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > max {
        let head: String = line.chars().take(max.saturating_sub(3)).collect();
        format!("{head}...")
    } else {
        line.to_string()
    }
}

fn launch_override(harness: &str, brief: &str) -> Option<Vec<String>> {
    let key = format!("DFLOW_LAUNCH_{}", harness.to_ascii_uppercase());
    let raw = std::env::var(key).ok()?;
    let argv = parse_argv(&raw)?;
    Some(substitute(argv.into_iter(), brief))
}

fn parse_argv(raw: &str) -> Option<Vec<String>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else if trimmed.starts_with('[') {
        serde_json::from_str::<Vec<String>>(trimmed).ok()
    } else {
        Some(trimmed.split_whitespace().map(str::to_string).collect())
    }
}

fn substitute(argv: impl Iterator<Item = String>, brief: &str) -> Vec<String> {
    argv.map(|tok| tok.replace("{brief}", brief)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_adapters_match_manifest_set_plus_custom() {
        // Every known adapter except `custom` must have a bundled manifest, and vice
        // versa, so validation and launch never drift.
        for a in KNOWN_ADAPTERS {
            if *a == "custom" {
                continue;
            }
            assert!(is_known_adapter(a), "{a} should be known");
            assert!(bundled_manifests().get(a).is_some(), "{a} needs a manifest");
        }
        assert!(is_known_adapter("custom"));
        assert!(!is_known_adapter("gemini"));
        for name in bundled_manifests().names() {
            assert!(KNOWN_ADAPTERS.contains(&name), "manifest {name} missing from KNOWN_ADAPTERS");
        }
    }

    #[test]
    fn claude_is_default_and_carries_autonomy_flag() {
        let cmd = harness_command("claude", "Fix login", None, None).unwrap();
        assert_eq!(cmd[0], "claude");
        assert!(cmd.contains(&"--permission-mode".to_string()));
        assert!(cmd.contains(&"acceptEdits".to_string()));
        assert_eq!(cmd.last().unwrap(), "Fix login");
    }

    #[test]
    fn model_and_effort_axes_reach_the_argv() {
        // The Phase 2 fix: model/effort now actually flow onto the launch line.
        let cmd = harness_command("claude", "b", Some("haiku"), Some("high")).unwrap();
        assert!(cmd.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(cmd.windows(2).any(|w| w == ["--effort", "high"]));
    }

    #[test]
    fn codex_and_opencode_shapes() {
        assert_eq!(harness_command("codex", "b", None, None).unwrap(), vec!["codex", "b"]);
        assert_eq!(
            harness_command("opencode", "b", None, None).unwrap(),
            vec!["opencode", "--prompt", "b"]
        );
    }

    #[test]
    fn unknown_harness_is_none() {
        assert!(harness_command("nope", "b", None, None).is_none());
    }

    #[test]
    fn brief_composition_and_preview() {
        assert_eq!(compose_brief("Title", None), "Title");
        assert_eq!(compose_brief("Title", Some("body")), "Title\n\nbody");
        assert_eq!(preview("first line\nsecond", 100), "first line");
        assert_eq!(preview("abcdefghij", 6), "abc...");
    }

    #[test]
    fn built_in_harness_no_longer_launches_unverified_families() {
        // cursor/pi are reachable only via a configured launcher, not the legacy path.
        assert!(harness_command("cursor", "b", None, None).is_none());
        assert!(harness_command("pi", "b", None, None).is_none());
    }

    #[test]
    fn launcher_argv_appends_extra_args_after_brief() {
        // cc-alt: a second claude subscription. Manifest flags precede the brief; the
        // launcher's own command replaces argv[0]; extra_args land last.
        let argv = launcher_command(
            "cc-alt",
            "claude",
            "claude",
            "Fix login",
            &["--dangerously-skip-permissions".to_string()],
            None,
            None,
        );
        assert_eq!(
            argv,
            vec![
                "claude",
                "--permission-mode",
                "acceptEdits",
                "Fix login",
                "--dangerously-skip-permissions",
            ]
        );
    }

    #[test]
    fn launcher_argv_keeps_opencode_prompt_value_adjacent() {
        let argv = launcher_command(
            "oc",
            "opencode",
            "opencode",
            "brief",
            &["--extra".to_string()],
            None,
            None,
        );
        // --prompt must stay immediately before the brief; extra args follow.
        assert_eq!(argv, vec!["opencode", "--prompt", "brief", "--extra"]);
    }

    #[test]
    fn launcher_argv_custom_adapter_has_no_flags() {
        let argv = launcher_command("mine", "custom", "my-cli", "brief", &[], None, None);
        assert_eq!(argv, vec!["my-cli", "brief"]);
    }

    #[test]
    fn launcher_override_replaces_base_but_keeps_extra_args() {
        // SAFETY: sets/removes a process-global env var; no other test reads this key.
        std::env::set_var("DFLOW_LAUNCH_STUBBY", r#"["cmd.exe","/k","echo {brief}"]"#);
        let argv =
            launcher_command("stubby", "claude", "claude", "hello", &["--x".to_string()], None, None);
        std::env::remove_var("DFLOW_LAUNCH_STUBBY");
        assert_eq!(argv, vec!["cmd.exe", "/k", "echo hello", "--x"]);
    }

    #[test]
    fn interactive_launcher_argv_has_no_brief() {
        let argv = launcher_interactive_command("claude", "claude", &["--x".to_string()], None, None);
        assert_eq!(argv, vec!["claude", "--permission-mode", "acceptEdits", "--x"]);
    }

    #[test]
    fn resume_command_from_manifest() {
        assert_eq!(
            resume_command("claude", "claude", "sess-1"),
            Some(vec!["claude".to_string(), "--resume".to_string(), "sess-1".to_string()])
        );
        // An unknown adapter has no manifest, so no resume argv.
        assert_eq!(resume_command("gemini", "gemini", "x"), None);
    }
}
