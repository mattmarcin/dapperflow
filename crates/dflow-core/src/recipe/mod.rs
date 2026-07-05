//! The flow-recipe engine (`recipes.md`).
//!
//! A recipe is a single markdown file with YAML front matter: the front matter is the
//! machine-enforced half (stages, plan/implement/verify/ship blocks, mcp mounts,
//! budgets), the body is stage-tagged natural-language guidance injected into agent
//! briefs. This module owns parsing, precise validation, `extends` inheritance, trust
//! classification (`security.md` / Recipe trust tiers), and content hashing. Scoping and
//! resolution across bundled/user/project scopes live in [`scope`]; the compiled-in
//! bundled set in [`bundled`]; the SQLite index and privilege grants in
//! `store::recipes`.
//!
//! Design rule, from `recipes.md` / Validation and safety: "invalid recipes never
//! partially apply". Parsing is all-or-nothing: [`Recipe::parse`] returns a fully
//! validated recipe or a single precise [`RecipeError`], and nothing downstream reads a
//! half-parsed value.

mod bundled;
mod scope;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::recipe::yaml::Node;

pub use bundled::{bundled_recipe_sources, BUNDLED_RECIPES};
pub use scope::{
    project_recipe_dir, CatalogEntry, InvalidEntry, RecipeCatalog, RecipeScope, ResolveError,
    ResolvedRecipe, ShadowedRecipe, PROJECT_SUBDIR,
};

pub mod yaml;

/// The fixed stage vocabulary (`recipes.md`: "The stage vocabulary is fixed").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Shape,
    Plan,
    Implement,
    Verify,
    Ship,
}

impl Stage {
    /// The wire/front-matter token for this stage.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Shape => "shape",
            Stage::Plan => "plan",
            Stage::Implement => "implement",
            Stage::Verify => "verify",
            Stage::Ship => "ship",
        }
    }

    /// Parse a stage token, or `None` if it is not in the fixed vocabulary.
    pub fn parse(token: &str) -> Option<Stage> {
        match token.trim().to_ascii_lowercase().as_str() {
            "shape" => Some(Stage::Shape),
            "plan" => Some(Stage::Plan),
            "implement" => Some(Stage::Implement),
            "verify" => Some(Stage::Verify),
            "ship" => Some(Stage::Ship),
            _ => None,
        }
    }

    /// Position in the canonical pipeline order, for order validation.
    fn order(self) -> u8 {
        match self {
            Stage::Shape => 0,
            Stage::Plan => 1,
            Stage::Implement => 2,
            Stage::Verify => 3,
            Stage::Ship => 4,
        }
    }

    /// The fixed vocabulary, in canonical order, for error messages.
    pub const VOCAB: &'static str = "shape, plan, implement, verify, ship";
}

/// The plan-stage artifact mode (`recipes.md` plan block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    Artifact,
    Markdown,
    None,
}

/// Whether the plan stage requires human approval before implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    Required,
    Auto,
}

/// Worktree strategy for the implement stage (`recipes.md` implement block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeStrategy {
    /// Reuse a warm pool slot (the default; standard tier).
    Pooled,
    /// Always lease a brand-new worktree (standard tier).
    Fresh,
    /// Work directly in the project checkout (privileged; requires a grant and an ack).
    InPlace,
}

impl WorktreeStrategy {
    /// The front-matter/wire token for this strategy.
    pub fn as_str(self) -> &'static str {
        match self {
            WorktreeStrategy::Pooled => "pooled",
            WorktreeStrategy::Fresh => "fresh",
            WorktreeStrategy::InPlace => "in_place",
        }
    }
}

/// Gate strictness for the verify stage (`recipes.md` verify block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStrictness {
    Full,
    ChecksOnly,
    None,
}

/// Ship target for the ship stage (`recipes.md` ship block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShipTarget {
    Pr,
    LocalMerge,
    None,
}

/// The recipe trust tier (`security.md` / Recipe trust tiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Standard,
    Privileged,
}

/// The `plan:` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanBlock {
    pub mode: PlanMode,
    pub approval: Approval,
    #[serde(default)]
    pub playbooks: Vec<String>,
}

/// The `implement:` block. Axes hold `None` for the `default` sentinel, meaning "defer
/// to the dispatch parameter or project default" (`recipes.md` / What recipes control).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImplementBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub worktree: WorktreeStrategy,
}

impl Default for ImplementBlock {
    fn default() -> Self {
        ImplementBlock { harness: None, model: None, effort: None, worktree: WorktreeStrategy::Pooled }
    }
}

/// One extra MCP server mounted into this recipe's sessions (`recipes.md` mcp block).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpMount {
    pub name: String,
    pub command: String,
    /// Stages the server is mounted for; empty means every stage.
    #[serde(default)]
    pub stages: Vec<Stage>,
}

/// The `verify:` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyBlock {
    pub gate: GateStrictness,
    /// `different` (a reviewer in a different family than the author) or a concrete
    /// adapter name (`recipes.md`).
    pub reviewer_harness: String,
}

/// The `ship:` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShipBlock {
    pub target: ShipTarget,
}

/// The `budgets:` block: engine-enforced per-session creation caps (`recipes.md` /
/// budgets, onboarding-audit.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cards: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<u32>,
}

/// One stage-tagged guidance section from the recipe body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Guidance {
    pub stage: Stage,
    /// The trimmed prose under the `## <stage>` heading.
    pub text: String,
}

/// A parsed, validated flow recipe (`recipes.md`).
///
/// `extends` is `Some` before inheritance is resolved and `None` after
/// ([`scope::RecipeCatalog::resolve`]); a resolved recipe carries the fully merged
/// fields ready for dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub name: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    pub stages: Vec<Stage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implement: Option<ImplementBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp: Vec<McpMount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<VerifyBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<ShipBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budgets: Option<Budgets>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guidance: Vec<Guidance>,
}

/// A single, precise recipe validation error. Carries the offending source line when it
/// is known, so `recipe.validate` can point the author at it (`recipes.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub struct RecipeError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

impl std::fmt::Display for RecipeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "line {l}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl RecipeError {
    fn msg(message: impl Into<String>) -> RecipeError {
        RecipeError { message: message.into(), line: None }
    }
}

/// One elevated capability a privileged recipe declares (`security.md` / Recipe trust
/// tiers). A per-project grant records exactly this list, and dispatch surfaces it in
/// the consent error, so the human sees precisely what they are approving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Elevation {
    /// Mounts an outside MCP server: the full command line is disclosed.
    McpMount { name: String, command: String },
    /// Works directly in the project checkout instead of an isolated worktree.
    WorktreeInPlace,
    /// Disables the gate while a ship stage is present (code could ship unreviewed).
    GateDisabledWithShip,
    /// Ships by merging locally rather than through a reviewable pull request.
    ShipLocalMerge,
}

impl Elevation {
    /// A one-line human description for the consent surface.
    pub fn describe(&self) -> String {
        match self {
            Elevation::McpMount { name, command } => {
                format!("mounts MCP server '{name}' (command: {command})")
            }
            Elevation::WorktreeInPlace => {
                "edits the project checkout in place (worktree: in_place)".to_string()
            }
            Elevation::GateDisabledWithShip => {
                "disables the verification gate with a ship stage present".to_string()
            }
            Elevation::ShipLocalMerge => "ships by local merge instead of a pull request".to_string(),
        }
    }
}

impl Recipe {
    /// Parse and fully validate a recipe from its markdown text. `name_hint` (usually the
    /// file stem) supplies the name only when the front matter omits `name`.
    ///
    /// All-or-nothing: any schema violation returns a single precise [`RecipeError`] and
    /// no partial recipe (`recipes.md` / Validation and safety).
    pub fn parse(name_hint: &str, text: &str) -> Result<Recipe, RecipeError> {
        let (front, body) = split_front_matter(text)?;
        // Front-matter lines are numbered from 1; the opening `---` fence is file line 1,
        // so file-relative line = front-matter line + 1. Report file-relative so the
        // author can jump straight to the offending line.
        let root = yaml::parse(front)
            .map_err(|e| RecipeError { message: e.message, line: Some(e.line + 1) })?;
        let mut recipe = interpret(name_hint, &root)?;
        recipe.guidance = extract_guidance(body);
        Ok(recipe)
    }

    /// Whether the recipe runs the given stage.
    pub fn has_stage(&self, stage: Stage) -> bool {
        self.stages.contains(&stage)
    }

    /// The worktree strategy (implement block, defaulting to pooled).
    pub fn worktree_strategy(&self) -> WorktreeStrategy {
        self.implement.as_ref().map(|i| i.worktree).unwrap_or(WorktreeStrategy::Pooled)
    }

    /// The harness axis, or `None` for the `default` sentinel.
    pub fn harness_axis(&self) -> Option<&str> {
        self.implement.as_ref().and_then(|i| i.harness.as_deref())
    }

    /// The model axis, or `None` for the `default` sentinel.
    pub fn model_axis(&self) -> Option<&str> {
        self.implement.as_ref().and_then(|i| i.model.as_deref())
    }

    /// The effort axis, or `None` for the `default` sentinel.
    pub fn effort_axis(&self) -> Option<&str> {
        self.implement.as_ref().and_then(|i| i.effort.as_deref())
    }

    /// The gate strictness, or `None` when the recipe declares no verify block.
    pub fn gate(&self) -> Option<GateStrictness> {
        self.verify.as_ref().map(|v| v.gate)
    }

    /// The guidance text for a stage, if the body tags one.
    pub fn guidance_for(&self, stage: Stage) -> Option<&str> {
        self.guidance.iter().find(|g| g.stage == stage).map(|g| g.text.as_str())
    }

    /// The full list of elevated capabilities this recipe declares (`security.md`).
    pub fn elevations(&self) -> Vec<Elevation> {
        let mut out = Vec::new();
        for mount in &self.mcp {
            out.push(Elevation::McpMount { name: mount.name.clone(), command: mount.command.clone() });
        }
        if self.worktree_strategy() == WorktreeStrategy::InPlace {
            out.push(Elevation::WorktreeInPlace);
        }
        // gate: none is only an elevation when a ship stage can actually ship code.
        if self.gate() == Some(GateStrictness::None) && self.has_stage(Stage::Ship) {
            out.push(Elevation::GateDisabledWithShip);
        }
        if self.ship.as_ref().map(|s| s.target) == Some(ShipTarget::LocalMerge) {
            out.push(Elevation::ShipLocalMerge);
        }
        out
    }

    /// The trust tier (`security.md`): privileged when any elevation is present, else
    /// standard. A shipless `gate: none` (the audit case) stays standard.
    pub fn trust_tier(&self) -> TrustTier {
        if self.elevations().is_empty() {
            TrustTier::Standard
        } else {
            TrustTier::Privileged
        }
    }

    /// Shallow-merge this recipe over `parent` for `extends` inheritance (`recipes.md`:
    /// "overrides merge shallowly"). Top-level blocks present in the child replace the
    /// parent's wholesale; absent ones inherit. Guidance merges per stage tag, so a child
    /// can deepen one stage's prose without restating the rest.
    fn merge_over(&self, parent: &Recipe) -> Recipe {
        let mut merged = parent.clone();
        merged.name = self.name.clone();
        merged.version = self.version;
        merged.extends = None;
        if self.description.is_some() {
            merged.description = self.description.clone();
        }
        // A child that lists its own stages wins; one that omits them (relying on
        // `extends`) inherits the parent's.
        if !self.stages.is_empty() {
            merged.stages = self.stages.clone();
        }
        if self.plan.is_some() {
            merged.plan = self.plan.clone();
        }
        if self.implement.is_some() {
            merged.implement = self.implement.clone();
        }
        if !self.mcp.is_empty() {
            merged.mcp = self.mcp.clone();
        }
        if self.verify.is_some() {
            merged.verify = self.verify.clone();
        }
        if self.ship.is_some() {
            merged.ship = self.ship.clone();
        }
        if self.budgets.is_some() {
            merged.budgets = self.budgets;
        }
        // Guidance: child sections override the parent's by stage; inherited stages the
        // child does not mention are kept.
        for g in &self.guidance {
            if let Some(slot) = merged.guidance.iter_mut().find(|x| x.stage == g.stage) {
                slot.text = g.text.clone();
            } else {
                merged.guidance.push(g.clone());
            }
        }
        merged
    }
}

/// The SHA-1 hex digest of a recipe file's text, used to invalidate a privilege grant
/// when the file changes (`security.md`: "re-confirmed when the recipe file's hash
/// changes"). A change detector, not a security primitive; the grant is the control.
pub fn content_hash(text: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Split a recipe file into its YAML front matter and its markdown body. The file must
/// open with a `---` fence (`recipes.md` / Format).
fn split_front_matter(text: &str) -> Result<(&str, &str), RecipeError> {
    let normalized = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = normalized
        .strip_prefix("---\n")
        .or_else(|| normalized.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            RecipeError::msg("a recipe must begin with YAML front matter fenced by '---'")
        })?;
    // Find the closing fence on its own line.
    let mut idx = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" {
            let front = &rest[..idx];
            let body = &rest[idx + line.len()..];
            return Ok((front, body));
        }
        idx += line.len();
    }
    Err(RecipeError::msg("front matter is not closed by a '---' fence"))
}

/// Extract stage-tagged guidance from the body: the prose under each `## <stage>`
/// heading whose text names a stage. Non-stage sections are left out of guidance.
fn extract_guidance(body: &str) -> Vec<Guidance> {
    let mut out: Vec<Guidance> = Vec::new();
    let mut current: Option<Stage> = None;
    let mut buf: Vec<&str> = Vec::new();
    let flush = |stage: Option<Stage>, buf: &mut Vec<&str>, out: &mut Vec<Guidance>| {
        if let Some(stage) = stage {
            let text = buf.join("\n").trim().to_string();
            if !text.is_empty() {
                out.push(Guidance { stage, text });
            }
        }
        buf.clear();
    };
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            flush(current, &mut buf, &mut out);
            current = Stage::parse(heading.trim());
        } else {
            buf.push(line);
        }
    }
    flush(current, &mut buf, &mut out);
    out
}

/// Interpret a parsed front-matter [`Node`] tree into a validated [`Recipe`] (before
/// inheritance). Every unknown/invalid value returns a precise error.
fn interpret(name_hint: &str, root: &Node) -> Result<Recipe, RecipeError> {
    let map = match root {
        Node::Map(_) => root,
        other => {
            return Err(RecipeError::msg(format!(
                "front matter must be a mapping, found a {}",
                other.kind()
            )))
        }
    };

    let name = match scalar_field(map, "name")? {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => {
            let hint = name_hint.trim();
            if hint.is_empty() {
                return Err(RecipeError::msg("recipe is missing a 'name'"));
            }
            hint.to_string()
        }
    };

    let version = match scalar_field(map, "version")? {
        Some(v) => v
            .trim()
            .parse::<u32>()
            .map_err(|_| RecipeError::msg(format!("version must be a positive integer, found '{v}'")))?,
        None => return Err(RecipeError::msg("recipe is missing a 'version'")),
    };
    if version < 1 {
        return Err(RecipeError::msg("version must be at least 1"));
    }

    let description = scalar_field(map, "description")?.filter(|d| !d.trim().is_empty());
    let extends = scalar_field(map, "extends")?.filter(|e| !e.trim().is_empty());

    // `stages` is required, except that a recipe with `extends` may inherit it (the
    // audit-deep case): an absent stage list stays empty here and is filled by merge.
    let stages = match map.get("stages") {
        Some(node) => parse_stages(node)?,
        None if extends.is_some() => Vec::new(),
        None => return Err(RecipeError::msg("recipe is missing a 'stages' list")),
    };
    let plan = parse_plan(map)?;
    let implement = parse_implement(map)?;
    let mcp = parse_mcp(map)?;
    let verify = parse_verify(map)?;
    let ship = parse_ship(map)?;
    let budgets = parse_budgets(map)?;

    Ok(Recipe {
        name,
        version,
        description,
        extends,
        stages,
        plan,
        implement,
        mcp,
        verify,
        ship,
        budgets,
        guidance: Vec::new(),
    })
}

/// Read a scalar field, erroring if the key holds a non-scalar node.
fn scalar_field(map: &Node, key: &str) -> Result<Option<String>, RecipeError> {
    match map.get(key) {
        None => Ok(None),
        Some(Node::Scalar(s)) => Ok(Some(s.clone())),
        Some(other) => Err(RecipeError::msg(format!("'{key}' must be a scalar, found a {}", other.kind()))),
    }
}

/// Parse and validate the `stages` list node: known vocabulary, no duplicates,
/// canonical pipeline order.
fn parse_stages(node: &Node) -> Result<Vec<Stage>, RecipeError> {
    let items = node
        .as_list()
        .ok_or_else(|| RecipeError::msg(format!("'stages' must be a list, e.g. [{}]", Stage::VOCAB)))?;
    if items.is_empty() {
        return Err(RecipeError::msg("'stages' must name at least one stage"));
    }
    let mut stages: Vec<Stage> = Vec::with_capacity(items.len());
    for item in items {
        let token = item
            .as_scalar()
            .ok_or_else(|| RecipeError::msg("each stage must be a name, not a nested value"))?;
        let stage = Stage::parse(token).ok_or_else(|| {
            RecipeError::msg(format!("unknown stage '{token}' (allowed: {})", Stage::VOCAB))
        })?;
        if stages.contains(&stage) {
            return Err(RecipeError::msg(format!("stage '{}' is listed more than once", stage.as_str())));
        }
        if let Some(prev) = stages.last() {
            if stage.order() <= prev.order() {
                return Err(RecipeError::msg(format!(
                    "stages must be in pipeline order ({}); '{}' cannot follow '{}'",
                    Stage::VOCAB,
                    stage.as_str(),
                    prev.as_str()
                )));
            }
        }
        stages.push(stage);
    }
    Ok(stages)
}

/// Parse the optional `plan:` block.
fn parse_plan(map: &Node) -> Result<Option<PlanBlock>, RecipeError> {
    let Some(block) = map.get("plan") else { return Ok(None) };
    let block = expect_map(block, "plan")?;
    let mode = match scalar_field(block, "mode")?.as_deref() {
        Some("artifact") => PlanMode::Artifact,
        Some("markdown") => PlanMode::Markdown,
        Some("none") => PlanMode::None,
        None => PlanMode::Artifact,
        Some(other) => {
            return Err(RecipeError::msg(format!(
                "plan.mode must be artifact|markdown|none, found '{other}'"
            )))
        }
    };
    let approval = match scalar_field(block, "approval")?.as_deref() {
        Some("required") => Approval::Required,
        Some("auto") => Approval::Auto,
        None => Approval::Required,
        Some(other) => {
            return Err(RecipeError::msg(format!("plan.approval must be required|auto, found '{other}'")))
        }
    };
    let playbooks = string_list(block, "playbooks")?;
    Ok(Some(PlanBlock { mode, approval, playbooks }))
}

/// Parse the optional `implement:` block, mapping the `default` axis sentinel to `None`.
fn parse_implement(map: &Node) -> Result<Option<ImplementBlock>, RecipeError> {
    let Some(block) = map.get("implement") else { return Ok(None) };
    let block = expect_map(block, "implement")?;
    let worktree = match scalar_field(block, "worktree")?.as_deref() {
        Some("pooled") => WorktreeStrategy::Pooled,
        Some("fresh") => WorktreeStrategy::Fresh,
        Some("in_place") => WorktreeStrategy::InPlace,
        None => WorktreeStrategy::Pooled,
        Some(other) => {
            return Err(RecipeError::msg(format!(
                "implement.worktree must be pooled|fresh|in_place, found '{other}'"
            )))
        }
    };
    Ok(Some(ImplementBlock {
        harness: axis(block, "harness")?,
        model: axis(block, "model")?,
        effort: axis(block, "effort")?,
        worktree,
    }))
}

/// Read an axis field: the `default` sentinel (or an empty value) becomes `None`.
fn axis(block: &Node, key: &str) -> Result<Option<String>, RecipeError> {
    Ok(scalar_field(block, key)?
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v != "default"))
}

/// Parse the optional `mcp:` list of mounts.
fn parse_mcp(map: &Node) -> Result<Vec<McpMount>, RecipeError> {
    let Some(node) = map.get("mcp") else { return Ok(Vec::new()) };
    let items = node
        .as_list()
        .ok_or_else(|| RecipeError::msg("'mcp' must be a list of { name, command, stages }"))?;
    let mut mounts = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let item = expect_map(item, &format!("mcp[{i}]"))?;
        let name = scalar_field(item, "name")?
            .filter(|n| !n.trim().is_empty())
            .ok_or_else(|| RecipeError::msg(format!("mcp[{i}] is missing a 'name'")))?;
        let command = scalar_field(item, "command")?
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| RecipeError::msg(format!("mcp mount '{name}' is missing a 'command'")))?;
        let stages = parse_stage_list(item, "stages", &format!("mcp mount '{name}'"))?;
        mounts.push(McpMount { name: name.trim().to_string(), command: command.trim().to_string(), stages });
    }
    Ok(mounts)
}

/// Parse the optional `verify:` block.
fn parse_verify(map: &Node) -> Result<Option<VerifyBlock>, RecipeError> {
    let Some(block) = map.get("verify") else { return Ok(None) };
    let block = expect_map(block, "verify")?;
    let gate = match scalar_field(block, "gate")?.as_deref() {
        Some("full") => GateStrictness::Full,
        Some("checks_only") => GateStrictness::ChecksOnly,
        Some("none") => GateStrictness::None,
        None => GateStrictness::Full,
        Some(other) => {
            return Err(RecipeError::msg(format!(
                "verify.gate must be full|checks_only|none, found '{other}'"
            )))
        }
    };
    let reviewer_harness =
        scalar_field(block, "reviewer_harness")?.unwrap_or_else(|| "different".to_string());
    Ok(Some(VerifyBlock { gate, reviewer_harness }))
}

/// Parse the optional `ship:` block.
fn parse_ship(map: &Node) -> Result<Option<ShipBlock>, RecipeError> {
    let Some(block) = map.get("ship") else { return Ok(None) };
    let block = expect_map(block, "ship")?;
    let target = match scalar_field(block, "target")?.as_deref() {
        Some("pr") => ShipTarget::Pr,
        Some("local_merge") => ShipTarget::LocalMerge,
        Some("none") => ShipTarget::None,
        None => ShipTarget::Pr,
        Some(other) => {
            return Err(RecipeError::msg(format!("ship.target must be pr|local_merge|none, found '{other}'")))
        }
    };
    Ok(Some(ShipBlock { target }))
}

/// Parse the optional `budgets:` block.
fn parse_budgets(map: &Node) -> Result<Option<Budgets>, RecipeError> {
    let Some(block) = map.get("budgets") else { return Ok(None) };
    let block = expect_map(block, "budgets")?;
    Ok(Some(Budgets { cards: count_field(block, "cards")?, notes: count_field(block, "notes")? }))
}

/// Read a non-negative integer budget field.
fn count_field(block: &Node, key: &str) -> Result<Option<u32>, RecipeError> {
    match scalar_field(block, key)? {
        None => Ok(None),
        Some(v) => v
            .trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| RecipeError::msg(format!("budgets.{key} must be a non-negative integer, found '{v}'"))),
    }
}

/// Coerce a node to a map, or a precise "expected a mapping" error.
fn expect_map<'a>(node: &'a Node, label: &str) -> Result<&'a Node, RecipeError> {
    match node {
        Node::Map(_) => Ok(node),
        other => Err(RecipeError::msg(format!("'{label}' must be a mapping, found a {}", other.kind()))),
    }
}

/// Read an optional list-of-strings field (e.g. `playbooks`).
fn string_list(block: &Node, key: &str) -> Result<Vec<String>, RecipeError> {
    match block.get(key) {
        None => Ok(Vec::new()),
        Some(Node::List(items)) => items
            .iter()
            .map(|i| {
                i.as_scalar()
                    .map(|s| s.to_string())
                    .ok_or_else(|| RecipeError::msg(format!("each item of '{key}' must be a scalar")))
            })
            .collect(),
        Some(other) => Err(RecipeError::msg(format!("'{key}' must be a list, found a {}", other.kind()))),
    }
}

/// Parse an optional stage-list field (e.g. an mcp mount's `stages`), validating the
/// vocabulary. An absent field means "all stages" and returns empty.
fn parse_stage_list(block: &Node, key: &str, label: &str) -> Result<Vec<Stage>, RecipeError> {
    match block.get(key) {
        None => Ok(Vec::new()),
        Some(Node::List(items)) => {
            let mut stages = Vec::with_capacity(items.len());
            for item in items {
                let token = item
                    .as_scalar()
                    .ok_or_else(|| RecipeError::msg(format!("{label} stages must be stage names")))?;
                let stage = Stage::parse(token).ok_or_else(|| {
                    RecipeError::msg(format!("{label}: unknown stage '{token}' (allowed: {})", Stage::VOCAB))
                })?;
                stages.push(stage);
            }
            Ok(stages)
        }
        Some(other) => {
            Err(RecipeError::msg(format!("{label} 'stages' must be a list, found a {}", other.kind())))
        }
    }
}
