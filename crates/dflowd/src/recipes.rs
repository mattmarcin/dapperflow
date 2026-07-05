//! Daemon-side recipe glue (`recipes.md` / Engine integration, `security.md` / Recipe
//! trust tiers, `protocol.md` / recipe.*).
//!
//! The engine (parser, validation, inheritance, scoping) lives in
//! `dflow_core::recipe`; this module binds it to the daemon: building the catalog from
//! the data dir and a project's checkout, resolving the dispatch recipe with the
//! card > project > global precedence, enforcing privilege grants with a structured
//! consent error, validating recipe x harness MCP compatibility, serving the recipe.*
//! protocol verbs, and rebuilding the SQLite index (file is truth, DB is index).

use std::path::{Path, PathBuf};

use dflow_core::recipe::{RecipeCatalog, RecipeScope, ResolveError};
use dflow_core::{bundled_manifests, Recipe, RecipeIndexRow, ResolvedRecipe, TrustTier};
use dflow_proto::{
    Card, ConsentRequired, Project, ProtocolError, RecipeGet, RecipeGetResult, RecipeGrant,
    RecipeGranted, RecipeGrantRevoked, RecipeInstall, RecipeInstalled, RecipeInvalid, RecipeList,
    RecipeListResult, RecipeRevokeGrant, RecipeSummary, RecipeValidate, RecipeValidateResult,
    RecipeValidationError,
};

use crate::api::store_err;
use crate::server::AppState;

/// The global default recipe when neither the card nor the project selects one
/// (`recipes.md` / Resolution and scoping).
pub const DEFAULT_RECIPE: &str = "standard";

/// Build the recipe catalog for an optional project: bundled (compiled-in) plus the
/// user dir (`<app-data>/recipes/`) plus, when a project is given, its
/// `<project>/.dapperflow/recipes/` (`recipes.md` / Resolution and scoping).
pub fn catalog_for(state: &AppState, project: Option<&Project>) -> RecipeCatalog {
    let user_dir = state.data_dir.recipes_dir();
    let project_dir: Option<PathBuf> =
        project.map(|p| dflow_core::project_recipe_dir(Path::new(&p.path)));
    RecipeCatalog::build(Some(&user_dir), project_dir.as_deref())
}

/// Resolve the recipe a dispatch runs under (`recipes.md`: card selection > project
/// `default_recipe` > global default `standard`; an explicit request parameter wins
/// over all three) and enforce its trust tier: a privileged recipe without a valid
/// per-project grant returns a `consent_required` error whose detail carries a
/// [`ConsentRequired`] payload the UI can turn into a consent flow.
pub fn resolve_dispatch_recipe(
    state: &AppState,
    requested: Option<&str>,
    card: &Card,
    project: &Project,
) -> Result<ResolvedRecipe, ProtocolError> {
    let name = requested
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| card.dial_recipe.clone().filter(|s| !s.trim().is_empty()))
        .or_else(|| project.default_recipe.clone().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_RECIPE.to_string());

    let catalog = catalog_for(state, Some(project));
    let resolved = catalog.resolve(&name).map_err(resolve_err)?;

    if resolved.trust_tier == TrustTier::Privileged {
        ensure_privilege_grant(state, project, &resolved)?;
    }
    Ok(resolved)
}

/// Enforce the per-project grant for a privileged recipe (`security.md`): the grant
/// must exist and its recorded hash must match the current file hash. The refusal is a
/// structured error, not a prompt; consent flows live in the UI.
fn ensure_privilege_grant(
    state: &AppState,
    project: &Project,
    resolved: &ResolvedRecipe,
) -> Result<(), ProtocolError> {
    let name = &resolved.recipe.name;
    let grant = state.store.recipe_grant(&project.id, name).map_err(store_err)?;
    let reason = match &grant {
        None => "no_grant",
        Some(g) if g.recipe_hash != resolved.hash => "hash_changed",
        Some(_) => return Ok(()),
    };
    let elevations: Vec<String> =
        resolved.recipe.elevations().iter().map(|e| e.describe()).collect();
    let detail = ConsentRequired {
        recipe_name: name.clone(),
        project_id: project.id.clone(),
        recipe_hash: resolved.hash.clone(),
        trust_tier: "privileged".to_string(),
        elevations: elevations.clone(),
        reason: reason.to_string(),
    };
    let message = match reason {
        "hash_changed" => format!(
            "recipe '{name}' is privileged and its file changed since it was granted; re-confirm the grant (elevates: {})",
            elevations.join("; ")
        ),
        _ => format!(
            "recipe '{name}' is privileged and has no grant on this project (elevates: {})",
            elevations.join("; ")
        ),
    };
    let detail_json = serde_json::to_string(&detail).unwrap_or_default();
    Err(ProtocolError::consent_required(message).with_detail(detail_json))
}

/// Validate recipe x harness MCP compatibility at dispatch time (`recipes.md` /
/// Validation and safety; `adapters.md` capability matrix): a recipe that mounts MCP
/// servers fails on a harness whose manifest does not verify MCP support (pi rejects,
/// and so does an unmanifested custom family, conservatively).
pub fn validate_mcp_for_harness(recipe: &Recipe, harness: &str) -> Result<(), ProtocolError> {
    if recipe.mcp.is_empty() {
        return Ok(());
    }
    let supported = bundled_manifests().get(harness).is_some_and(|m| m.capabilities.mcp);
    if supported {
        Ok(())
    } else {
        let mounts: Vec<&str> = recipe.mcp.iter().map(|m| m.name.as_str()).collect();
        Err(ProtocolError::bad_request(format!(
            "recipe '{}' mounts MCP servers ({}) but harness '{harness}' has no verified MCP support; \
             pick an MCP-capable harness or a recipe without mounts",
            recipe.name,
            mounts.join(", ")
        )))
    }
}

// ---- recipe.* protocol verbs ----

/// `recipe.list { project_id? }`: every winning recipe across scopes with its source,
/// trust tier, and what it shadows, plus files that failed to parse (surfaced, never
/// silently missing).
pub fn recipe_list(state: &AppState, req: RecipeList) -> Result<RecipeListResult, ProtocolError> {
    let project = lookup_project(state, req.project_id.as_deref())?;
    let catalog = catalog_for(state, project.as_ref());
    let mut recipes = Vec::new();
    for entry in catalog.winners() {
        // Summaries reflect the fully resolved recipe (inheritance applied) so the
        // listed tier matches what dispatch enforces; a resolution failure (broken
        // extends) falls back to the pre-merge parse and is also surfaced as invalid.
        let (recipe, tier) = match catalog.resolve(&entry.name) {
            Ok(r) => (r.recipe, r.trust_tier),
            Err(_) => (entry.recipe.clone(), entry.recipe.trust_tier()),
        };
        recipes.push(RecipeSummary {
            name: entry.name.clone(),
            scope: entry.scope.as_str().to_string(),
            version: recipe.version,
            description: recipe.description.clone(),
            trust_tier: tier_str(tier).to_string(),
            source_path: entry.source_path.clone(),
            shadowed_scopes: catalog
                .shadowed_scopes(&entry.name)
                .into_iter()
                .map(|s| s.as_str().to_string())
                .collect(),
            elevations: recipe.elevations().iter().map(|e| e.describe()).collect(),
        });
    }
    let mut invalid: Vec<RecipeInvalid> = catalog
        .invalid()
        .iter()
        .map(|i| RecipeInvalid {
            scope: i.scope.as_str().to_string(),
            source_path: i.source_path.clone(),
            name_hint: i.name_hint.clone(),
            error: RecipeValidationError { message: i.error.message.clone(), line: i.error.line },
        })
        .collect();
    // A winner whose extends chain is broken is unusable; report it alongside parse
    // failures so the list is honest about what will actually dispatch.
    for entry in catalog.winners() {
        if let Err(err) = catalog.resolve(&entry.name) {
            invalid.push(RecipeInvalid {
                scope: entry.scope.as_str().to_string(),
                source_path: entry.source_path.clone(),
                name_hint: entry.name.clone(),
                error: RecipeValidationError { message: err.to_string(), line: None },
            });
        }
    }
    Ok(RecipeListResult { recipes, invalid })
}

/// `recipe.get { name, project_id? }`: the resolved recipe (inheritance applied) with
/// its summary and full parsed structure.
pub fn recipe_get(state: &AppState, req: RecipeGet) -> Result<RecipeGetResult, ProtocolError> {
    let project = lookup_project(state, req.project_id.as_deref())?;
    let catalog = catalog_for(state, project.as_ref());
    match catalog.resolve(req.name.trim()) {
        Ok(resolved) => {
            let summary = RecipeSummary {
                name: resolved.recipe.name.clone(),
                scope: resolved.scope.as_str().to_string(),
                version: resolved.recipe.version,
                description: resolved.recipe.description.clone(),
                trust_tier: tier_str(resolved.trust_tier).to_string(),
                source_path: resolved.source_path.clone(),
                shadowed_scopes: catalog
                    .shadowed_scopes(&resolved.recipe.name)
                    .into_iter()
                    .map(|s| s.as_str().to_string())
                    .collect(),
                elevations: resolved.recipe.elevations().iter().map(|e| e.describe()).collect(),
            };
            let parsed = serde_json::to_value(&resolved.recipe).ok();
            Ok(RecipeGetResult { found: true, summary: Some(summary), parsed, errors: Vec::new() })
        }
        Err(err) => Ok(RecipeGetResult {
            found: false,
            summary: None,
            parsed: None,
            errors: vec![RecipeValidationError { message: err.to_string(), line: None }],
        }),
    }
}

/// `recipe.validate { content, name? }`: parse arbitrary recipe text and report precise
/// errors without installing anything (`recipes.md` / Validation and safety).
pub fn recipe_validate(req: RecipeValidate) -> RecipeValidateResult {
    let hint = req.name.as_deref().unwrap_or("recipe");
    match Recipe::parse(hint, &req.content) {
        Ok(recipe) => RecipeValidateResult {
            valid: true,
            trust_tier: Some(tier_str(recipe.trust_tier()).to_string()),
            elevations: recipe.elevations().iter().map(|e| e.describe()).collect(),
            parsed: serde_json::to_value(&recipe).ok(),
            errors: Vec::new(),
        },
        Err(err) => RecipeValidateResult {
            valid: false,
            parsed: None,
            trust_tier: None,
            elevations: Vec::new(),
            errors: vec![RecipeValidationError { message: err.message, line: err.line }],
        },
    }
}

/// `recipe.install { source, scope, project_id?, content? }`: validate FIRST, then copy
/// the file into the scope dir and rebuild the index. An invalid recipe is never
/// written anywhere ("invalid recipes never partially apply"); nothing in a recipe
/// executes at install time, it is inert text.
pub fn recipe_install(state: &AppState, req: RecipeInstall) -> Result<RecipeInstalled, ProtocolError> {
    let text = match &req.content {
        Some(inline) => inline.clone(),
        None => std::fs::read_to_string(&req.source).map_err(|e| {
            ProtocolError::bad_request(format!("cannot read recipe source '{}': {e}", req.source))
        })?,
    };
    let stem = Path::new(&req.source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recipe")
        .to_string();
    let recipe = Recipe::parse(&stem, &text)
        .map_err(|e| ProtocolError::bad_request(format!("invalid recipe: {e}")))?;

    let dir = match req.scope.as_str() {
        "user" => state.data_dir.recipes_dir(),
        "project" => {
            let pid = req.project_id.as_deref().ok_or_else(|| {
                ProtocolError::bad_request("recipe.install with scope 'project' needs a project_id")
            })?;
            let project = state
                .store
                .get_project(pid)
                .map_err(store_err)?
                .ok_or_else(|| ProtocolError::not_found(format!("project {pid}")))?;
            dflow_core::project_recipe_dir(Path::new(&project.path))
        }
        other => {
            return Err(ProtocolError::bad_request(format!(
                "scope must be 'user' or 'project', got '{other}' (bundled recipes ship with the app)"
            )))
        }
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| ProtocolError::internal(format!("creating {}: {e}", dir.display())))?;
    let target = dir.join(format!("{}.md", recipe.name));
    std::fs::write(&target, &text)
        .map_err(|e| ProtocolError::internal(format!("writing {}: {e}", target.display())))?;

    // File changed: rebuild the index so the DB projection stays fresh.
    rebuild_recipe_index(state);

    Ok(RecipeInstalled {
        name: recipe.name.clone(),
        scope: req.scope,
        path: target.to_string_lossy().into_owned(),
        trust_tier: tier_str(recipe.trust_tier()).to_string(),
    })
}

/// `recipe.grant { project_id, recipe_name }`: record per-project consent for a
/// privileged recipe at its CURRENT file hash (`security.md`). Granting a standard
/// recipe is refused: there is nothing to elevate, and issuing no-op grants trains
/// users to click through consent.
pub fn recipe_grant(state: &AppState, req: RecipeGrant) -> Result<RecipeGranted, ProtocolError> {
    let project = state
        .store
        .get_project(&req.project_id)
        .map_err(store_err)?
        .ok_or_else(|| ProtocolError::not_found(format!("project {}", req.project_id)))?;
    let catalog = catalog_for(state, Some(&project));
    let resolved = catalog.resolve(req.recipe_name.trim()).map_err(resolve_err)?;
    if resolved.trust_tier != TrustTier::Privileged {
        return Err(ProtocolError::bad_request(format!(
            "recipe '{}' is standard tier; it needs no grant",
            resolved.recipe.name
        )));
    }
    let elevations: Vec<String> =
        resolved.recipe.elevations().iter().map(|e| e.describe()).collect();
    let elevations_json = serde_json::to_string(&resolved.recipe.elevations())
        .map_err(|e| ProtocolError::internal(e.to_string()))?;
    state
        .store
        .grant_recipe(&project.id, &resolved.recipe.name, &resolved.hash, &elevations_json)
        .map_err(store_err)?;
    Ok(RecipeGranted {
        project_id: project.id,
        recipe_name: resolved.recipe.name,
        recipe_hash: resolved.hash,
        elevations,
    })
}

/// `recipe.revoke_grant { project_id, recipe_name }`.
pub fn recipe_revoke_grant(
    state: &AppState,
    req: RecipeRevokeGrant,
) -> Result<RecipeGrantRevoked, ProtocolError> {
    let revoked =
        state.store.revoke_recipe_grant(&req.project_id, &req.recipe_name).map_err(store_err)?;
    Ok(RecipeGrantRevoked { revoked })
}

// ---- SQLite index (file is truth, DB is index) ----

/// Rebuild the whole `recipes` index from disk: bundled + user scopes once, plus every
/// registered project's `.dapperflow/recipes/` (`data-model.md`: rebuilt on daemon
/// start and on file change/install). Failures are logged, never fatal: the index is a
/// projection, and dispatch reads the files directly.
pub fn rebuild_recipe_index(state: &AppState) {
    let mut rows: Vec<RecipeIndexRow> = Vec::new();

    let global = catalog_for(state, None);
    push_catalog_rows(&global, None, &mut rows, &[RecipeScope::Bundled, RecipeScope::User]);

    match state.store.list_projects() {
        Ok(projects) => {
            for project in projects {
                let catalog = catalog_for(state, Some(&project));
                push_catalog_rows(&catalog, Some(&project.id), &mut rows, &[RecipeScope::Project]);
            }
        }
        Err(err) => tracing::warn!(%err, "could not list projects for recipe index rebuild"),
    }

    if let Err(err) = state.store.replace_recipe_index(&rows) {
        tracing::warn!(%err, "recipe index rebuild failed");
    }
}

/// Append index rows for a catalog's entries in the given scopes. The cached `parsed`
/// JSON is the resolved (inheritance-applied) recipe when resolvable, else the
/// pre-merge parse, so the index never goes empty over a broken extends chain.
fn push_catalog_rows(
    catalog: &RecipeCatalog,
    project_id: Option<&str>,
    rows: &mut Vec<RecipeIndexRow>,
    scopes: &[RecipeScope],
) {
    for entry in catalog.winners() {
        if !scopes.contains(&entry.scope) {
            continue;
        }
        let (recipe, tier) = match catalog.resolve(&entry.name) {
            Ok(r) => (r.recipe, r.trust_tier),
            Err(_) => (entry.recipe.clone(), entry.recipe.trust_tier()),
        };
        rows.push(RecipeIndexRow {
            name: entry.name.clone(),
            scope: entry.scope.as_str().to_string(),
            project_id: project_id.map(str::to_string),
            source_path: entry.source_path.clone(),
            parsed_json: serde_json::to_string(&recipe).ok(),
            hash: Some(entry.hash.clone()),
            trust_tier: Some(tier_str(tier).to_string()),
        });
    }
}

// ---- helpers ----

fn tier_str(tier: TrustTier) -> &'static str {
    match tier {
        TrustTier::Standard => "standard",
        TrustTier::Privileged => "privileged",
    }
}

fn resolve_err(err: ResolveError) -> ProtocolError {
    match err {
        ResolveError::NotFound(name) => ProtocolError::not_found(format!(
            "no recipe named '{name}' in any scope (bundled, user, project)"
        )),
        other => ProtocolError::bad_request(other.to_string()),
    }
}

fn lookup_project(
    state: &AppState,
    project_id: Option<&str>,
) -> Result<Option<Project>, ProtocolError> {
    match project_id {
        None => Ok(None),
        Some(pid) => Ok(Some(
            state
                .store
                .get_project(pid)
                .map_err(store_err)?
                .ok_or_else(|| ProtocolError::not_found(format!("project {pid}")))?,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp_recipe() -> Recipe {
        Recipe::parse(
            "t",
            "---\nname: t\nversion: 1\nstages: [implement]\nmcp:\n  - name: context7\n    command: \"npx -y @upstash/context7-mcp\"\n---\n",
        )
        .unwrap()
    }

    /// pi ships no MCP by design, so an MCP-mounting recipe must fail recipe x harness
    /// validation at dispatch (`adapters.md` capability matrix; `recipes.md`).
    #[test]
    fn mcp_recipe_rejected_on_pi_and_unknown_harness() {
        let recipe = mcp_recipe();
        let err = validate_mcp_for_harness(&recipe, "pi").unwrap_err();
        assert!(err.message.contains("no verified MCP support"), "got: {}", err.message);
        // An unmanifested family is rejected conservatively too.
        assert!(validate_mcp_for_harness(&recipe, "stub").is_err());
    }

    #[test]
    fn mcp_recipe_allowed_on_mcp_capable_harnesses() {
        let recipe = mcp_recipe();
        for harness in ["claude", "codex", "opencode"] {
            assert!(
                validate_mcp_for_harness(&recipe, harness).is_ok(),
                "{harness} verifies MCP support in its manifest"
            );
        }
    }

    /// A recipe without mounts passes on every harness: there is nothing to validate.
    #[test]
    fn mountless_recipe_passes_everywhere() {
        let recipe =
            Recipe::parse("t", "---\nname: t\nversion: 1\nstages: [implement]\n---\n").unwrap();
        assert!(validate_mcp_for_harness(&recipe, "pi").is_ok());
        assert!(validate_mcp_for_harness(&recipe, "stub").is_ok());
    }
}
