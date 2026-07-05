//! Adapter manifests: the data that defines a harness behavior family
//! (`adapters.md` / Adapter manifests).
//!
//! One TOML file per harness in `adapters/` at the repo root. This module parses,
//! validates, and serves them, replacing the built-in harness table from Phase 1.5
//! (`harness.rs`). The five launch-set manifests are compiled in via `include_str!`
//! so the daemon always ships a valid set; `ManifestSet::load_dir` additionally reads
//! an on-disk `adapters/` directory for development and future user overrides.
//!
//! "Adding a harness must be data plus a probe run, never an engine change"
//! (`adapters.md`): the launch line, autonomy/model/effort flags, busy signature,
//! composer hints, dialog rules, and controls are all manifest fields, not code.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

/// A parsed, validated adapter manifest (`adapters.md` / Adapter manifests).
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub adapter: AdapterSection,
    #[serde(default)]
    pub signals: SignalsSection,
    #[serde(default)]
    pub controls: ControlsSection,
    #[serde(default)]
    pub dialogs: DialogsSection,
    #[serde(default)]
    pub composer: ComposerSection,
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    #[serde(default)]
    pub context_injection: ContextInjectionSection,
}

/// `[adapter]`: identity plus the launch line and its flag arrays.
#[derive(Debug, Clone, Deserialize)]
pub struct AdapterSection {
    pub name: String,
    pub command: String,
    /// Launch template. Tokens: `{command}`, `{autonomy_flags}`, `{model_flag}`,
    /// `{effort_flag}`, `{prompt}`. See [`Manifest::build_launch`].
    #[serde(default)]
    pub launch: Vec<String>,
    #[serde(default)]
    pub autonomy_flags: Vec<String>,
    /// Model-axis flag with a `{model}` placeholder (spliced only when a model is set).
    #[serde(default)]
    pub model_flag: Vec<String>,
    /// Effort-axis flag with an `{effort}` placeholder (spliced only when effort is set).
    #[serde(default)]
    pub effort_flag: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// When true, automated steering is refused for this harness with a Needs You
    /// explanation rather than attempted blind (`adapters.md` / Verified submit).
    #[serde(default)]
    pub no_auto_steer: bool,
}

/// `[signals]`: the tier-3 busy signature and the tier-2 native mechanism name.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SignalsSection {
    #[serde(default)]
    pub busy_signature: String,
    /// Free-form tier-2 mechanism tag (`hooks`, `notify`, `sse`, `none`), for docs.
    #[serde(default)]
    pub native: String,
}

/// `[controls]`: interrupt/exit/resume/skill forms.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ControlsSection {
    #[serde(default)]
    pub interrupt: String,
    #[serde(default)]
    pub exit_command: String,
    /// Resume argv template with a `{command}` and `{resume_ref}` placeholder.
    #[serde(default)]
    pub resume: Vec<String>,
    #[serde(default)]
    pub skill_invocation: String,
}

/// `[dialogs]`: trust/permission prompt rules matched on the screen model.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DialogsSection {
    #[serde(default)]
    pub trust: Option<DialogRule>,
}

/// A `pattern -> response` dialog rule (`adapters.md` / dialogs).
#[derive(Debug, Clone, Deserialize)]
pub struct DialogRule {
    pub pattern: String,
    pub response: String,
}

/// `[composer]`: hints for verified submit (`adapters.md` / Verified submit).
#[derive(Debug, Clone, Deserialize)]
pub struct ComposerSection {
    /// Prefixes that open a completion popup which can swallow the first Enter.
    #[serde(default)]
    pub popup_prefixes: Vec<String>,
    /// How long to wait for a popup to settle before pressing Enter.
    #[serde(default = "default_settle_ms")]
    pub popup_settle_ms: u64,
    /// Cell styles that mark ghost/placeholder text to strip during classification.
    #[serde(default)]
    pub ghost_text_styles: Vec<String>,
}

impl Default for ComposerSection {
    fn default() -> Self {
        Self {
            popup_prefixes: Vec::new(),
            popup_settle_ms: default_settle_ms(),
            ghost_text_styles: Vec::new(),
        }
    }
}

fn default_settle_ms() -> u64 {
    1200
}

/// `[capabilities]`: audit-derived capability facts (`adapters.md` / capability matrix).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CapabilitiesSection {
    /// Whether the harness supports MCP mounts. `false` (e.g. pi) means an
    /// MCP-mounting recipe must fail recipe x harness validation at dispatch.
    #[serde(default)]
    pub mcp: bool,
    #[serde(default)]
    pub native_tier2: String,
}

/// `[context_injection]`: how this harness receives the standing `dflow` usage guidance
/// as ambient context for a session with no composed brief (a New Session), injected the
/// least-intrusive way the harness allows and NEVER by writing into the user's project
/// checkout (`adapters.md` / Standing-guidance injection).
///
/// The method is resolved per harness by the adapter probe and recorded here as data, so
/// adding or changing a harness is a manifest edit, not an engine change.
#[derive(Debug, Clone, Deserialize)]
pub struct ContextInjectionSection {
    /// One of [`CI_APPEND_SYSTEM_PROMPT`], [`CI_FIRST_PROMPT`], or [`CI_NONE`]:
    /// - `append_system_prompt`: splice [`Self::flag`] (with `{guidance}` replaced) into
    ///   the launch argv - a session-scoped, non-polluting system-prompt append (the
    ///   preferred mechanism, e.g. Claude Code `--append-system-prompt`).
    /// - `first_prompt`: the harness has no session-scoped system-prompt flag and only a
    ///   global (per-user) or repo instructions file, so the guidance is prepended to the
    ///   session's first prompt instead - documented as degraded (it rides in the visible
    ///   conversation, never in the user's checkout).
    /// - `none`: no non-polluting mechanism at all, so a New Session on this harness
    ///   launches WITHOUT standing guidance rather than writing into the user's repo.
    #[serde(default = "default_ci_method")]
    pub method: String,
    /// For `append_system_prompt`: the flag pair spliced into argv, with a `{guidance}`
    /// placeholder replaced by the standing-guidance text (e.g.
    /// `["--append-system-prompt", "{guidance}"]`).
    #[serde(default)]
    pub flag: Vec<String>,
}

/// `context_injection.method`: a session-scoped launch flag appends the guidance to the
/// system prompt (preferred; no repo pollution, works for a New Session with no brief).
pub const CI_APPEND_SYSTEM_PROMPT: &str = "append_system_prompt";
/// `context_injection.method`: no system-prompt flag; prepend the guidance to the first
/// prompt (degraded, documented). Non-polluting: it never touches the user's checkout.
pub const CI_FIRST_PROMPT: &str = "first_prompt";
/// `context_injection.method`: no non-polluting mechanism; New Session launches without
/// standing guidance (the harness is flagged guidance-unsupported).
pub const CI_NONE: &str = "none";

/// Every accepted `context_injection.method` value.
const CI_METHODS: &[&str] = &[CI_APPEND_SYSTEM_PROMPT, CI_FIRST_PROMPT, CI_NONE];

impl Default for ContextInjectionSection {
    fn default() -> Self {
        Self { method: default_ci_method(), flag: Vec::new() }
    }
}

fn default_ci_method() -> String {
    CI_NONE.to_string()
}

/// Errors parsing or validating a manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest {name}: {message}")]
    Invalid { name: String, message: String },
    #[error("parsing {name}: {source}")]
    Parse { name: String, source: toml::de::Error },
    #[error("reading {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("duplicate adapter name '{0}' across manifests")]
    Duplicate(String),
}

impl Manifest {
    /// Parse and validate a manifest from TOML text. `label` names it in errors.
    pub fn parse(label: &str, text: &str) -> Result<Manifest, ManifestError> {
        let manifest: Manifest = toml::from_str(text)
            .map_err(|source| ManifestError::Parse { name: label.to_string(), source })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validate structural invariants a launch depends on.
    fn validate(&self) -> Result<(), ManifestError> {
        let name = self.adapter.name.trim();
        if name.is_empty() {
            return Err(self.invalid("adapter.name must not be empty"));
        }
        if self.adapter.command.trim().is_empty() {
            return Err(self.invalid("adapter.command must not be empty"));
        }
        if self.adapter.launch.is_empty() {
            return Err(self.invalid("adapter.launch must not be empty"));
        }
        if !self.adapter.launch.iter().any(|t| t == "{command}") {
            return Err(self.invalid("adapter.launch must contain a {command} token"));
        }
        if !self.adapter.model_flag.is_empty()
            && !self.adapter.model_flag.iter().any(|t| t.contains("{model}"))
        {
            return Err(self.invalid("adapter.model_flag must contain a {model} placeholder"));
        }
        if !self.adapter.effort_flag.is_empty()
            && !self.adapter.effort_flag.iter().any(|t| t.contains("{effort}"))
        {
            return Err(self.invalid("adapter.effort_flag must contain an {effort} placeholder"));
        }
        if self.composer.popup_settle_ms == 0 {
            return Err(self.invalid("composer.popup_settle_ms must be greater than zero"));
        }
        let ci_method = self.context_injection.method.as_str();
        if !CI_METHODS.contains(&ci_method) {
            return Err(self.invalid(&format!(
                "context_injection.method must be one of {CI_METHODS:?}, got '{ci_method}'"
            )));
        }
        if ci_method == CI_APPEND_SYSTEM_PROMPT {
            if self.context_injection.flag.is_empty() {
                return Err(
                    self.invalid("context_injection.flag must be non-empty for append_system_prompt")
                );
            }
            if !self.context_injection.flag.iter().any(|t| t.contains("{guidance}")) {
                return Err(self.invalid(
                    "context_injection.flag must contain a {guidance} placeholder for append_system_prompt",
                ));
            }
        }
        Ok(())
    }

    fn invalid(&self, message: &str) -> ManifestError {
        ManifestError::Invalid { name: self.adapter.name.clone(), message: message.to_string() }
    }

    /// Expand the launch template into a concrete argv (`adapters.md` / dispatch flow).
    ///
    /// `command` is the launcher's own command (so `cc-alt` launching `claude` works).
    /// `prompt` is the composed brief, or `None` for a bare interactive session (the
    /// `{prompt}` token, and any value-consuming flag immediately before it, are then
    /// dropped so a flag like `--prompt` never dangles). `model`/`effort` splice their
    /// flag arrays only when set. A launcher's `extra_args` are appended by the caller.
    pub fn build_launch(
        &self,
        command: &str,
        prompt: Option<&str>,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Vec<String> {
        let mut argv: Vec<String> = Vec::with_capacity(self.adapter.launch.len() + 2);
        for token in &self.adapter.launch {
            match token.as_str() {
                "{command}" => argv.push(command.to_string()),
                "{autonomy_flags}" => argv.extend(self.adapter.autonomy_flags.iter().cloned()),
                "{model_flag}" => {
                    if let Some(m) = model.filter(|m| !m.is_empty()) {
                        argv.extend(self.adapter.model_flag.iter().map(|f| f.replace("{model}", m)));
                    }
                }
                "{effort_flag}" => {
                    if let Some(e) = effort.filter(|e| !e.is_empty()) {
                        argv.extend(
                            self.adapter.effort_flag.iter().map(|f| f.replace("{effort}", e)),
                        );
                    }
                }
                "{prompt}" => match prompt {
                    Some(p) => argv.push(p.to_string()),
                    None => {
                        // Interactive: drop a trailing value-consuming flag (e.g.
                        // opencode `--prompt`) that would otherwise dangle.
                        if argv.last().is_some_and(|t| t.starts_with('-')) {
                            argv.pop();
                        }
                    }
                },
                other => argv.push(other.to_string()),
            }
        }
        argv
    }

    /// Expand the resume argv template (`{command}`, `{resume_ref}`), or `None` when
    /// this manifest declares no resume mechanism.
    pub fn build_resume(&self, command: &str, resume_ref: &str) -> Option<Vec<String>> {
        if self.controls.resume.is_empty() {
            return None;
        }
        Some(
            self.controls
                .resume
                .iter()
                .map(|t| t.replace("{command}", command).replace("{resume_ref}", resume_ref))
                .collect(),
        )
    }

    /// The launch-argv flag that injects `guidance` into this harness's system prompt,
    /// with `{guidance}` filled in, or `None` when the harness does not use the
    /// `append_system_prompt` method (`adapters.md` / Standing-guidance injection).
    pub fn context_injection_flag(&self, guidance: &str) -> Option<Vec<String>> {
        if self.context_injection.method != CI_APPEND_SYSTEM_PROMPT {
            return None;
        }
        Some(self.context_injection.flag.iter().map(|t| t.replace("{guidance}", guidance)).collect())
    }

    /// This harness's context-injection method (`adapters.md`): one of
    /// [`CI_APPEND_SYSTEM_PROMPT`], [`CI_FIRST_PROMPT`], or [`CI_NONE`].
    pub fn context_injection_method(&self) -> &str {
        &self.context_injection.method
    }
}

/// A set of adapter manifests keyed by adapter name.
#[derive(Debug, Clone, Default)]
pub struct ManifestSet {
    by_name: BTreeMap<String, Manifest>,
}

/// The bundled launch-set manifests, compiled in from `adapters/` at the repo root.
const BUNDLED: &[(&str, &str)] = &[
    ("claude", include_str!("../../../adapters/claude.toml")),
    ("codex", include_str!("../../../adapters/codex.toml")),
    ("opencode", include_str!("../../../adapters/opencode.toml")),
    ("pi", include_str!("../../../adapters/pi.toml")),
    ("cursor", include_str!("../../../adapters/cursor.toml")),
];

impl ManifestSet {
    /// The compiled-in launch-set manifests. Panics only if a shipped manifest is
    /// malformed, which a unit test guards against at build time.
    pub fn bundled() -> Result<ManifestSet, ManifestError> {
        let mut set = ManifestSet::default();
        for (label, text) in BUNDLED {
            let manifest = Manifest::parse(label, text)?;
            set.insert(manifest)?;
        }
        Ok(set)
    }

    /// Load every `*.toml` in a directory as a manifest set (development/override).
    pub fn load_dir(dir: &Path) -> Result<ManifestSet, ManifestError> {
        let mut set = ManifestSet::default();
        let entries = std::fs::read_dir(dir)
            .map_err(|source| ManifestError::Io { path: dir.display().to_string(), source })?;
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for path in files {
            let label = path.file_stem().and_then(|s| s.to_str()).unwrap_or("manifest").to_string();
            let text = std::fs::read_to_string(&path)
                .map_err(|source| ManifestError::Io { path: path.display().to_string(), source })?;
            let manifest = Manifest::parse(&label, &text)?;
            set.insert(manifest)?;
        }
        Ok(set)
    }

    fn insert(&mut self, manifest: Manifest) -> Result<(), ManifestError> {
        let name = manifest.adapter.name.clone();
        if self.by_name.contains_key(&name) {
            return Err(ManifestError::Duplicate(name));
        }
        self.by_name.insert(name, manifest);
        Ok(())
    }

    /// The manifest for `adapter`, if this set has one.
    pub fn get(&self, adapter: &str) -> Option<&Manifest> {
        self.by_name.get(adapter)
    }

    /// Adapter names in this set, sorted.
    pub fn names(&self) -> Vec<&str> {
        self.by_name.keys().map(String::as_str).collect()
    }

    /// Number of manifests in the set.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// The process-wide bundled manifest set (lazily parsed once).
///
/// Harness launch resolution and tier-3 heuristics read from here, so the manifests
/// are the single source of truth at runtime.
pub fn bundled_manifests() -> &'static ManifestSet {
    static SET: OnceLock<ManifestSet> = OnceLock::new();
    SET.get_or_init(|| ManifestSet::bundled().expect("bundled adapter manifests must be valid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifests_parse_and_validate() {
        let set = ManifestSet::bundled().expect("bundled manifests valid");
        // The five launch-set adapters (adapters.md).
        for name in ["claude", "codex", "opencode", "pi", "cursor"] {
            assert!(set.get(name).is_some(), "missing bundled manifest {name}");
        }
        assert_eq!(set.len(), 5);
    }

    #[test]
    fn claude_launch_expands_axes() {
        let set = ManifestSet::bundled().unwrap();
        let m = set.get("claude").unwrap();
        // Full dispatch: brief + model + effort.
        let argv = m.build_launch("claude", Some("Fix login"), Some("haiku"), Some("high"));
        assert_eq!(
            argv,
            vec![
                "claude",
                "--permission-mode",
                "acceptEdits",
                "--model",
                "haiku",
                "--effort",
                "high",
                "Fix login",
            ]
        );
    }

    #[test]
    fn claude_launch_without_axes_omits_flags() {
        let set = ManifestSet::bundled().unwrap();
        let m = set.get("claude").unwrap();
        let argv = m.build_launch("claude", Some("brief"), None, None);
        assert_eq!(argv, vec!["claude", "--permission-mode", "acceptEdits", "brief"]);
    }

    #[test]
    fn opencode_prompt_flag_stays_adjacent_and_drops_when_interactive() {
        let set = ManifestSet::bundled().unwrap();
        let m = set.get("opencode").unwrap();
        // Dispatch: --prompt immediately before the brief.
        let argv = m.build_launch("opencode", Some("brief"), None, None);
        assert_eq!(argv, vec!["opencode", "--prompt", "brief"]);
        // Interactive: the dangling --prompt is dropped.
        let interactive = m.build_launch("opencode", None, None, None);
        assert_eq!(interactive, vec!["opencode"]);
    }

    #[test]
    fn cursor_is_conservative_and_no_auto_steer() {
        let set = ManifestSet::bundled().unwrap();
        let m = set.get("cursor").unwrap();
        assert!(m.adapter.no_auto_steer, "cursor must refuse auto-steer until audited");
        assert_eq!(m.adapter.command, "cursor-agent", "cursor family launches cursor-agent, not the editor");
        // cursor-agent takes a positional prompt; a model axis splices --model.
        assert_eq!(m.build_launch("cursor-agent", Some("b"), None, None), vec!["cursor-agent", "b"]);
        assert_eq!(
            m.build_launch("cursor-agent", Some("b"), Some("gpt-5"), None),
            vec!["cursor-agent", "--model", "gpt-5", "b"]
        );
    }

    #[test]
    fn pi_declares_no_mcp() {
        let set = ManifestSet::bundled().unwrap();
        assert!(!set.get("pi").unwrap().capabilities.mcp, "pi ships no MCP by design");
        assert!(set.get("claude").unwrap().capabilities.mcp);
    }

    #[test]
    fn claude_resume_template_expands() {
        let set = ManifestSet::bundled().unwrap();
        let m = set.get("claude").unwrap();
        assert_eq!(
            m.build_resume("claude", "sess-123"),
            Some(vec!["claude".to_string(), "--resume".to_string(), "sess-123".to_string()])
        );
        // A manifest with no resume mechanism returns None.
        let no_resume = Manifest::parse(
            "nr",
            "[adapter]\nname=\"nr\"\ncommand=\"nr\"\nlaunch=[\"{command}\",\"{prompt}\"]\n",
        )
        .unwrap();
        assert_eq!(no_resume.build_resume("nr", "x"), None);
    }

    #[test]
    fn empty_launch_is_rejected() {
        let text = r#"
[adapter]
name = "bad"
command = "bad"
launch = []
"#;
        let err = Manifest::parse("bad", text).unwrap_err();
        assert!(matches!(err, ManifestError::Invalid { .. }));
    }

    #[test]
    fn launch_without_command_token_is_rejected() {
        let text = r#"
[adapter]
name = "bad"
command = "bad"
launch = ["oops", "{prompt}"]
"#;
        let err = Manifest::parse("bad", text).unwrap_err();
        assert!(matches!(err, ManifestError::Invalid { .. }));
    }

    #[test]
    fn duplicate_adapter_names_rejected() {
        let mut set = ManifestSet::default();
        let text = r#"
[adapter]
name = "dup"
command = "dup"
launch = ["{command}", "{prompt}"]
"#;
        set.insert(Manifest::parse("a", text).unwrap()).unwrap();
        let err = set.insert(Manifest::parse("b", text).unwrap()).unwrap_err();
        assert!(matches!(err, ManifestError::Duplicate(_)));
    }

    #[test]
    fn composer_hints_present_for_claude() {
        let set = ManifestSet::bundled().unwrap();
        let c = &set.get("claude").unwrap().composer;
        assert_eq!(c.popup_prefixes, vec!["/".to_string()]);
        assert!(c.popup_settle_ms > 0);
        assert!(c.ghost_text_styles.iter().any(|s| s == "dim"));
    }

    #[test]
    fn claude_context_injection_is_system_prompt_append() {
        let set = ManifestSet::bundled().unwrap();
        let claude = set.get("claude").unwrap();
        assert_eq!(claude.context_injection_method(), CI_APPEND_SYSTEM_PROMPT);
        // The flag is resolved with the guidance text spliced in, ready to splice into argv.
        assert_eq!(
            claude.context_injection_flag("USE DFLOW"),
            Some(vec!["--append-system-prompt".to_string(), "USE DFLOW".to_string()])
        );
    }

    #[test]
    fn fallback_harnesses_have_no_system_prompt_flag() {
        let set = ManifestSet::bundled().unwrap();
        // codex/opencode/pi offer no session-scoped system-prompt flag (only global or
        // repo instructions files, which would pollute), so they fall back to first_prompt.
        for name in ["codex", "opencode", "pi"] {
            let m = set.get(name).unwrap();
            assert_eq!(m.context_injection_method(), CI_FIRST_PROMPT, "{name}");
            assert_eq!(m.context_injection_flag("g"), None, "{name} has no append flag");
        }
        // cursor is unaudited (no_auto_steer): flagged guidance-unsupported for New Session.
        assert_eq!(set.get("cursor").unwrap().context_injection_method(), CI_NONE);
    }

    #[test]
    fn append_system_prompt_requires_a_guidance_placeholder() {
        // A manifest declaring append_system_prompt with no {guidance} placeholder is
        // rejected at parse time, so the flag can never launch empty.
        let text = r#"
[adapter]
name = "bad"
command = "bad"
launch = ["{command}", "{prompt}"]

[context_injection]
method = "append_system_prompt"
flag = ["--append-system-prompt", "static"]
"#;
        let err = Manifest::parse("bad", text).unwrap_err();
        assert!(matches!(err, ManifestError::Invalid { .. }));
    }

    #[test]
    fn unknown_context_injection_method_is_rejected() {
        let text = r#"
[adapter]
name = "bad"
command = "bad"
launch = ["{command}", "{prompt}"]

[context_injection]
method = "telepathy"
"#;
        let err = Manifest::parse("bad", text).unwrap_err();
        assert!(matches!(err, ManifestError::Invalid { .. }));
    }

    #[test]
    fn default_context_injection_method_is_none() {
        // A manifest with no [context_injection] section defaults to none (guidance-off),
        // never accidentally polluting or half-configuring a harness.
        let m = Manifest::parse(
            "x",
            "[adapter]\nname=\"x\"\ncommand=\"x\"\nlaunch=[\"{command}\",\"{prompt}\"]\n",
        )
        .unwrap();
        assert_eq!(m.context_injection_method(), CI_NONE);
        assert_eq!(m.context_injection_flag("g"), None);
    }
}
