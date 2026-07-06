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

use std::path::Path;

use crate::manifest::{
    bundled_manifests, Manifest, BRIEF_DELIVERY_ARGV, BRIEF_DELIVERY_TYPED,
};

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

/// Resolve the launch argv for a legacy built-in `harness`, substituting `prompt` and
/// the optional model/effort axes from the adapter manifest.
///
/// `prompt` is the composed brief for an argv-delivery harness, or `None` to leave the
/// brief out of the argv entirely (a typed-delivery harness types it after launch, so the
/// `{prompt}` slot - and any value flag before it - is dropped; see [`brief_delivery`]).
///
/// Taken when no configured launcher matches. Covers only the built-in dispatchable
/// families (claude/codex/opencode); other families need a configured launcher. A
/// `DFLOW_LAUNCH_<H>` override still fully replaces the argv (the test seam). Returns
/// `None` for an unknown/undispatchable harness with no override.
pub fn harness_command(
    harness: &str,
    prompt: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Option<Vec<String>> {
    if let Some(argv) = launch_override(harness, prompt) {
        return Some(argv);
    }
    if !BUILTIN_DISPATCHABLE.contains(&harness) {
        return None;
    }
    let manifest = bundled_manifests().get(harness)?;
    Some(manifest.build_launch(harness, prompt, model, effort))
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
    prompt: Option<&str>,
    extra_args: &[String],
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let mut argv = match launch_override(name, prompt) {
        Some(base) => base,
        None => manifest_launch(adapter, command, prompt, model, effort),
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

/// How the composed dispatch brief is delivered to a launched agent
/// (`adapters.md` / Dispatch brief delivery).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefDelivery {
    /// Pass the brief as the `{prompt}` launch argument (native exe; proven for claude).
    Argv,
    /// Type the brief after launch via the readiness-gated verified-submit path (a shim
    /// harness whose launch-argument brief `cmd.exe` would truncate at the first newline).
    Typed,
}

/// Decide how a dispatched harness receives its composed brief (`adapters.md` / Dispatch
/// brief delivery; audit finding on shim truncation).
///
/// `manifest` is the resolved adapter manifest (`None` for an unmanifested custom/stub
/// launcher, which cannot do verified submit and so keeps the argv path). `command` is the
/// resolved launch argv *before* it is rewritten for spawn - its `command[0]` determines
/// the launchable form.
///
/// Precedence:
/// - an explicit manifest `brief_delivery = "argv" | "typed"` wins (a data override);
/// - otherwise (`auto`, the default) the launchable form decides: a launch that resolves to
///   run through `cmd.exe /c` (a `.cmd`/`.bat` shim - the finding #2 case) truncates a
///   multi-line argument at the first newline, so the brief is typed; a native executable
///   keeps the proven argv path. This reuses the finding #2
///   [`crate::agents::launchable_command`] signal, so the decision tracks how the process
///   actually launches rather than a hardcoded per-name assumption.
pub fn brief_delivery(manifest: Option<&Manifest>, command: &[String]) -> BriefDelivery {
    match manifest.map(|m| m.adapter.brief_delivery.as_str()) {
        Some(BRIEF_DELIVERY_ARGV) => BriefDelivery::Argv,
        Some(BRIEF_DELIVERY_TYPED) => BriefDelivery::Typed,
        // `auto`: decide from the resolved launchable form.
        Some(_) => {
            if launches_via_cmd_exe(command) {
                BriefDelivery::Typed
            } else {
                BriefDelivery::Argv
            }
        }
        // No manifest (custom/stub launcher): no verified-submit path, so keep argv.
        None => BriefDelivery::Argv,
    }
}

/// Whether the resolved spawn form of `command` runs through `cmd.exe` - i.e. the launch
/// is a `.cmd`/`.bat` shim (or already targets `cmd.exe`), the case `cmd.exe /c` truncates
/// a multi-line argument on. Reuses the finding #2 launch resolver; on a non-Windows
/// target `launchable_command` is a pass-through, so this is always false (no `cmd.exe`,
/// no truncation).
fn launches_via_cmd_exe(command: &[String]) -> bool {
    let launchable = crate::agents::launchable_command(command);
    launchable.first().is_some_and(|program| {
        Path::new(program)
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(|f| f.eq_ignore_ascii_case("cmd.exe"))
    })
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

fn launch_override(harness: &str, prompt: Option<&str>) -> Option<Vec<String>> {
    let key = format!("DFLOW_LAUNCH_{}", harness.to_ascii_uppercase());
    let raw = std::env::var(key).ok()?;
    let argv = parse_argv(&raw)?;
    // A typed-delivery launch passes `None`, so the `{brief}` seam collapses to empty
    // (the brief is typed after launch, never embedded in argv).
    Some(substitute(argv.into_iter(), prompt.unwrap_or("")))
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
        let cmd = harness_command("claude", Some("Fix login"), None, None).unwrap();
        assert_eq!(cmd[0], "claude");
        assert!(cmd.contains(&"--permission-mode".to_string()));
        assert!(cmd.contains(&"acceptEdits".to_string()));
        assert_eq!(cmd.last().unwrap(), "Fix login");
    }

    #[test]
    fn model_and_effort_axes_reach_the_argv() {
        // The Phase 2 fix: model/effort now actually flow onto the launch line.
        let cmd = harness_command("claude", Some("b"), Some("haiku"), Some("high")).unwrap();
        assert!(cmd.windows(2).any(|w| w == ["--model", "haiku"]));
        assert!(cmd.windows(2).any(|w| w == ["--effort", "high"]));
    }

    #[test]
    fn codex_and_opencode_shapes() {
        assert_eq!(harness_command("codex", Some("b"), None, None).unwrap(), vec!["codex", "b"]);
        assert_eq!(
            harness_command("opencode", Some("b"), None, None).unwrap(),
            vec!["opencode", "--prompt", "b"]
        );
    }

    #[test]
    fn no_prompt_keeps_the_brief_out_of_argv() {
        // A typed-delivery harness resolves with `None`: the `{prompt}` slot - and the
        // dangling `--prompt` before it - are dropped, so nothing multi-line rides the argv.
        assert_eq!(harness_command("codex", None, None, None).unwrap(), vec!["codex"]);
        assert_eq!(harness_command("opencode", None, None, None).unwrap(), vec!["opencode"]);
        // Axes still splice; only the brief is absent.
        let cmd = harness_command("codex", None, Some("gpt-5"), None).unwrap();
        assert_eq!(cmd, vec!["codex", "-m", "gpt-5"]);
    }

    #[test]
    fn unknown_harness_is_none() {
        assert!(harness_command("nope", Some("b"), None, None).is_none());
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
        assert!(harness_command("cursor", Some("b"), None, None).is_none());
        assert!(harness_command("pi", Some("b"), None, None).is_none());
    }

    #[test]
    fn brief_delivery_honors_an_explicit_manifest_field() {
        let set = crate::manifest::ManifestSet::bundled().unwrap();
        // claude declares argv; the resolved command is irrelevant to an explicit field.
        assert_eq!(
            brief_delivery(set.get("claude"), &["claude".to_string(), "brief".to_string()]),
            BriefDelivery::Argv
        );
        // codex/opencode/pi declare typed, regardless of the resolved command shape.
        for name in ["codex", "opencode", "pi"] {
            assert_eq!(
                brief_delivery(set.get(name), &[name.to_string()]),
                BriefDelivery::Typed,
                "{name} declares typed delivery"
            );
        }
    }

    #[test]
    fn brief_delivery_without_a_manifest_is_argv() {
        // An unmanifested custom/stub launcher (no verified-submit path) keeps argv, even
        // when it launches through cmd.exe - this is why the cmd.exe stub dispatch test,
        // which has no manifest, still receives its brief as a launch argument.
        assert_eq!(
            brief_delivery(None, &["cmd.exe".to_string(), "/c".to_string(), "echo hi".to_string()]),
            BriefDelivery::Argv
        );
    }

    #[cfg(windows)]
    #[test]
    fn brief_delivery_auto_reads_the_launchable_form() {
        // An `auto` manifest decides from how the process actually launches: a `.cmd` shim
        // (resolved to run under cmd.exe) -> typed; a native `.exe` -> argv.
        let auto = Manifest::parse(
            "auto",
            "[adapter]\nname=\"auto\"\ncommand=\"auto\"\nlaunch=[\"{command}\",\"{prompt}\"]\nbrief_delivery=\"auto\"\n",
        )
        .unwrap();

        let dir = std::env::temp_dir().join(format!(
            "dflow-delivery-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shimtool.cmd"), "@echo hi\r\n").unwrap();
        std::fs::write(dir.join("nativetool.exe"), b"MZ").unwrap();

        // The decision function resolves command[0] on the real PATH, so point the probe at
        // absolute shim/exe paths under our temp dir (resolved verbatim, no PATH needed).
        let shim = dir.join("shimtool.cmd").to_string_lossy().into_owned();
        let native = dir.join("nativetool.exe").to_string_lossy().into_owned();
        assert_eq!(brief_delivery(Some(&auto), &[shim]), BriefDelivery::Typed);
        assert_eq!(brief_delivery(Some(&auto), &[native]), BriefDelivery::Argv);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launcher_argv_appends_extra_args_after_brief() {
        // cc-alt: a second claude subscription. Manifest flags precede the brief; the
        // launcher's own command replaces argv[0]; extra_args land last.
        let argv = launcher_command(
            "cc-alt",
            "claude",
            "claude",
            Some("Fix login"),
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
            Some("brief"),
            &["--extra".to_string()],
            None,
            None,
        );
        // --prompt must stay immediately before the brief; extra args follow.
        assert_eq!(argv, vec!["opencode", "--prompt", "brief", "--extra"]);
    }

    #[test]
    fn launcher_argv_custom_adapter_has_no_flags() {
        let argv = launcher_command("mine", "custom", "my-cli", Some("brief"), &[], None, None);
        assert_eq!(argv, vec!["my-cli", "brief"]);
    }

    #[test]
    fn launcher_argv_typed_delivery_omits_the_brief() {
        // A typed-delivery launcher resolves with `None`, so the brief never reaches argv;
        // opencode's `--prompt` is dropped rather than left dangling.
        let argv = launcher_command("oc", "opencode", "opencode", None, &["--x".to_string()], None, None);
        assert_eq!(argv, vec!["opencode", "--x"]);
    }

    #[test]
    fn launcher_override_replaces_base_but_keeps_extra_args() {
        // SAFETY: sets/removes a process-global env var; no other test reads this key.
        std::env::set_var("DFLOW_LAUNCH_STUBBY", r#"["cmd.exe","/k","echo {brief}"]"#);
        let argv = launcher_command(
            "stubby",
            "claude",
            "claude",
            Some("hello"),
            &["--x".to_string()],
            None,
            None,
        );
        // A typed-delivery resolution passes None, so the {brief} seam collapses to empty.
        let typed =
            launcher_command("stubby", "claude", "claude", None, &["--x".to_string()], None, None);
        std::env::remove_var("DFLOW_LAUNCH_STUBBY");
        assert_eq!(argv, vec!["cmd.exe", "/k", "echo hello", "--x"]);
        assert_eq!(typed, vec!["cmd.exe", "/k", "echo ", "--x"]);
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
