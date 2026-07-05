//! Recipe scoping and resolution (`recipes.md` / Resolution and scoping).
//!
//! Three scopes, most-specific first: bundled (shipped with the app) < user
//! (`<app-data>/recipes/`) < project (`<project>/.dapperflow/recipes/`). A name
//! collision resolves to the most specific scope, and the catalog records which files
//! were shadowed so the UI can always show which one won. `extends` is resolved through
//! the same precedence, so a project recipe can tweak one knob of a bundled recipe.
//!
//! The files are the truth (`data-model.md`): the catalog reads them fresh, and the
//! SQLite `recipes` table is a rebuilt-from-disk index, never a second source of truth.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{bundled_recipe_sources, content_hash, Recipe, RecipeError, TrustTier};

/// The subdirectory of a project that holds project-scoped recipes.
pub const PROJECT_SUBDIR: &str = ".dapperflow/recipes";

/// A recipe scope, ordered least- to most-specific for resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeScope {
    Bundled,
    User,
    Project,
}

impl RecipeScope {
    pub fn as_str(self) -> &'static str {
        match self {
            RecipeScope::Bundled => "bundled",
            RecipeScope::User => "user",
            RecipeScope::Project => "project",
        }
    }
}

/// One recipe source in the catalog: its scope, declared name, on-disk path (absent for
/// bundled), raw text, content hash, and the parsed (pre-inheritance) recipe.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub scope: RecipeScope,
    pub name: String,
    pub source_path: Option<String>,
    pub text: String,
    pub hash: String,
    pub recipe: Recipe,
}

/// A recipe file that failed to parse, kept out of resolution but surfaced for the UI so
/// a broken file is visible rather than silently missing.
#[derive(Debug, Clone)]
pub struct InvalidEntry {
    pub scope: RecipeScope,
    pub source_path: Option<String>,
    pub name_hint: String,
    pub error: RecipeError,
}

/// A resolved recipe: the fully inheritance-merged recipe plus the winning source's
/// scope, path, hash, and trust tier.
#[derive(Debug, Clone)]
pub struct ResolvedRecipe {
    pub recipe: Recipe,
    pub scope: RecipeScope,
    pub source_path: Option<String>,
    /// The winning file's content hash (invalidates a privilege grant when it changes).
    pub hash: String,
    pub trust_tier: TrustTier,
}

/// A recipe whose name shadows one or more less-specific files.
#[derive(Debug, Clone)]
pub struct ShadowedRecipe {
    pub name: String,
    pub winning_scope: RecipeScope,
    pub shadowed_scopes: Vec<RecipeScope>,
}

/// Errors resolving a recipe by name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("no recipe named '{0}' in any scope")]
    NotFound(String),
    #[error("recipe '{name}' extends '{parent}', which does not exist")]
    ExtendsNotFound { name: String, parent: String },
    #[error("recipe '{0}' is part of an extends cycle")]
    Cycle(String),
    #[error("recipe '{name}' is incomplete after inheritance: {message}")]
    Incomplete { name: String, message: String },
}

/// Max `extends` chain depth, a belt-and-suspenders bound alongside cycle detection.
const MAX_EXTENDS_DEPTH: usize = 16;

/// The resolved catalog across bundled, user, and project scopes.
pub struct RecipeCatalog {
    /// The winning entry per name (most-specific scope).
    by_name: BTreeMap<String, CatalogEntry>,
    /// Every source seen, including shadowed ones, for the "which won" surface.
    all: Vec<CatalogEntry>,
    /// Files that failed to parse, surfaced but not resolvable.
    invalid: Vec<InvalidEntry>,
}

impl RecipeCatalog {
    /// Build the catalog: bundled (compiled-in) < user dir < project dir. A missing dir
    /// contributes nothing; a malformed file is recorded as invalid, never fatal.
    pub fn build(user_dir: Option<&Path>, project_dir: Option<&Path>) -> RecipeCatalog {
        let mut catalog = RecipeCatalog { by_name: BTreeMap::new(), all: Vec::new(), invalid: Vec::new() };

        for (hint, text) in bundled_recipe_sources() {
            catalog.ingest(RecipeScope::Bundled, hint, text, None);
        }
        if let Some(dir) = user_dir {
            catalog.ingest_dir(RecipeScope::User, dir);
        }
        if let Some(dir) = project_dir {
            catalog.ingest_dir(RecipeScope::Project, dir);
        }
        catalog
    }

    /// Read every `*.md` in a directory (sorted, deterministic) as a scope's recipes.
    fn ingest_dir(&mut self, scope: RecipeScope, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return, // a missing scope dir is normal
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .collect();
        files.sort();
        for path in files {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let hint = path.file_stem().and_then(|s| s.to_str()).unwrap_or("recipe");
            let source = Some(path.to_string_lossy().into_owned());
            self.ingest(scope, hint, &text, source);
        }
    }

    /// Parse one source and place it in the catalog (or record it as invalid).
    fn ingest(&mut self, scope: RecipeScope, hint: &str, text: &str, source_path: Option<String>) {
        match Recipe::parse(hint, text) {
            Ok(recipe) => {
                let entry = CatalogEntry {
                    scope,
                    name: recipe.name.clone(),
                    source_path,
                    text: text.to_string(),
                    hash: content_hash(text),
                    recipe,
                };
                // More-specific scopes are ingested later, so they overwrite the winner.
                self.by_name.insert(entry.name.clone(), entry.clone());
                self.all.push(entry);
            }
            Err(error) => self.invalid.push(InvalidEntry {
                scope,
                source_path,
                name_hint: hint.to_string(),
                error,
            }),
        }
    }

    /// Resolve a recipe by name to its fully inheritance-merged form.
    pub fn resolve(&self, name: &str) -> Result<ResolvedRecipe, ResolveError> {
        let entry = self.by_name.get(name).ok_or_else(|| ResolveError::NotFound(name.to_string()))?;
        let mut visited: Vec<(RecipeScope, String)> = Vec::new();
        let recipe = self.resolve_entry(entry, &mut visited)?;
        if recipe.stages.is_empty() {
            return Err(ResolveError::Incomplete {
                name: name.to_string(),
                message: "no stages after resolving 'extends'".to_string(),
            });
        }
        let trust_tier = recipe.trust_tier();
        Ok(ResolvedRecipe {
            recipe,
            scope: entry.scope,
            source_path: entry.source_path.clone(),
            hash: entry.hash.clone(),
            trust_tier,
        })
    }

    /// Recursively resolve `extends`, with cycle and depth guards. The identity used for
    /// cycle detection is `(scope, name)`, so a project recipe may `extends` the same
    /// name in a less-specific scope (the "tweak one knob of a bundled recipe" case in
    /// `recipes.md`) without tripping the cycle guard.
    fn resolve_entry(
        &self,
        entry: &CatalogEntry,
        visited: &mut Vec<(RecipeScope, String)>,
    ) -> Result<Recipe, ResolveError> {
        let key = (entry.scope, entry.name.clone());
        if visited.contains(&key) || visited.len() > MAX_EXTENDS_DEPTH {
            return Err(ResolveError::Cycle(entry.name.clone()));
        }
        let Some(parent_name) = entry.recipe.extends.clone() else {
            return Ok(entry.recipe.clone());
        };
        visited.push(key);
        let parent_entry = self.select_parent(entry, &parent_name).ok_or_else(|| {
            ResolveError::ExtendsNotFound { name: entry.name.clone(), parent: parent_name.clone() }
        })?;
        let parent = self.resolve_entry(parent_entry, visited)?;
        Ok(entry.recipe.merge_over(&parent))
    }

    /// Choose the entry a child's `extends: parent_name` resolves to. When the parent
    /// name equals the child's own name (self-shadowing across scopes), the parent must
    /// come from a strictly less-specific scope; otherwise the most-specific match wins.
    fn select_parent(&self, entry: &CatalogEntry, parent_name: &str) -> Option<&CatalogEntry> {
        self.all
            .iter()
            .filter(|c| c.name == parent_name)
            .filter(|c| parent_name != entry.name || c.scope < entry.scope)
            .max_by_key(|c| c.scope)
    }

    /// Whether the catalog knows a recipe by this name (in any scope).
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// The winning entries, name-sorted.
    pub fn winners(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.by_name.values()
    }

    /// Files that failed to parse.
    pub fn invalid(&self) -> &[InvalidEntry] {
        &self.invalid
    }

    /// Scopes shadowed by the winner for `name` (less-specific files with the same name).
    pub fn shadowed_scopes(&self, name: &str) -> Vec<RecipeScope> {
        let winning = match self.by_name.get(name) {
            Some(e) => e.scope,
            None => return Vec::new(),
        };
        let mut scopes: Vec<RecipeScope> =
            self.all.iter().filter(|e| e.name == name && e.scope != winning).map(|e| e.scope).collect();
        scopes.sort();
        scopes.dedup();
        scopes
    }

    /// Shadow reports for every winner that shadows at least one other file.
    pub fn shadowed(&self) -> Vec<ShadowedRecipe> {
        self.by_name
            .values()
            .filter_map(|entry| {
                let shadowed = self.shadowed_scopes(&entry.name);
                if shadowed.is_empty() {
                    None
                } else {
                    Some(ShadowedRecipe {
                        name: entry.name.clone(),
                        winning_scope: entry.scope,
                        shadowed_scopes: shadowed,
                    })
                }
            })
            .collect()
    }
}

/// The project-scoped recipe directory for a project root, if it exists.
pub fn project_recipe_dir(project_root: &Path) -> PathBuf {
    project_root.join(PROJECT_SUBDIR)
}
