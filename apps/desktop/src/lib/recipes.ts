// Flow-recipe presentation helpers (recipes.md / security.md trust tiers). Pure:
// no fixture data, no daemon. The dial and the consent summary read these.

import { PrivilegeKind, Recipe, RecipeGrantError, TrustTier } from "../model";

export const TRUST_META: Record<TrustTier, { label: string; tone: string; blurb: string }> = {
  standard: {
    label: "Standard",
    tone: "standard",
    blurb: "Runs with no extra consent: pooled worktrees, the full gate, guidance only.",
  },
  privileged: {
    label: "Privileged",
    tone: "privileged",
    blurb: "Elevates execution or delivery. Needs a per-project grant before it can run.",
  },
};

// Human name for each elevated capability, for the consent summary (security.md:
// the grant "lists exactly what is elevated").
export const PRIVILEGE_LABEL: Record<PrivilegeKind, string> = {
  mcp: "Mounts an outside MCP server",
  worktree_in_place: "Edits your working tree in place",
  gate_disabled: "Ships with the verification gate off",
  local_merge: "Merges to your local branch without a PR",
};

// The grant identity: a grant is scoped to project + recipe + content hash, so a
// hash change (edited recipe file) invalidates it and re-prompts (security.md).
export function grantKey(projectId: string, recipeName: string, contentHash: string): string {
  return `${projectId}::${recipeName}::${contentHash}`;
}

// Build the structured error the daemon returns for an ungranted privileged recipe.
// The dial simulates the daemon here in fixture mode; live mode surfaces the real one.
export function recipeGrantError(recipe: Recipe, projectId: string): RecipeGrantError {
  return {
    code: "recipe_grant_required",
    message: `The "${recipe.name}" recipe is privileged and needs a grant for this project.`,
    recipe: recipe.name,
    project_id: projectId,
    privileges: recipe.privileges,
    contentHash: recipe.contentHash,
  };
}

// A recipe needs a grant when it is privileged and its (project, hash) is not in the
// granted set. Cards with no project fall back to the global default at dispatch, so
// a privileged recipe cannot be granted until a project is chosen.
export function needsGrant(
  recipe: Recipe | undefined,
  projectId: string | null,
  granted: ReadonlySet<string>,
): boolean {
  if (!recipe || recipe.trust !== "privileged") return false;
  if (!projectId) return true;
  return !granted.has(grantKey(projectId, recipe.name, recipe.contentHash));
}

export function findRecipe(recipes: Recipe[], name: string | null | undefined): Recipe | undefined {
  if (!name) return undefined;
  return recipes.find((r) => r.name === name);
}

// The effective recipe for a card: card dial > project default > global default
// (recipes.md resolution order).
export function effectiveRecipeName(
  cardDial: string | null | undefined,
  projectDefault: string | null | undefined,
): string {
  return cardDial ?? projectDefault ?? "standard";
}

// --- Grant persistence -------------------------------------------------------
// Per-project grants for privileged recipes, keyed project::recipe::hash so an
// edited recipe file re-prompts (security.md). The daemon will own these once the
// recipes crate serves grants; localStorage keeps the UI honest until then.

const GRANTS_KEY = "dflow.recipe-grants.v1";

export function loadGrants(): Set<string> {
  try {
    const raw = window.localStorage.getItem(GRANTS_KEY);
    if (!raw) return new Set();
    const arr = JSON.parse(raw) as unknown;
    return new Set(Array.isArray(arr) ? arr.filter((x): x is string => typeof x === "string") : []);
  } catch {
    return new Set();
  }
}

export function saveGrants(grants: ReadonlySet<string>): void {
  try {
    window.localStorage.setItem(GRANTS_KEY, JSON.stringify([...grants]));
  } catch {
    /* storage unavailable: grants live for the session only */
  }
}

// Marker error a dispatch throws when it parked itself behind the consent modal,
// so callers can skip their failure toast (the modal is the continuation).
export const GRANT_PENDING_MESSAGE = "recipe_grant_required";

export function isGrantPending(e: unknown): boolean {
  return e instanceof Error && e.message === GRANT_PENDING_MESSAGE;
}
