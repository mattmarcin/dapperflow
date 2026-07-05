// The process dial (product.md): every card carries a dial that selects a flow
// recipe. The dial reads recipe.list (bundled + user + project, with the winning
// source shown), renders each recipe's stage list and trust tier, and walks the
// consent summary for privileged recipes (security.md / Recipe trust tiers).

import { useEffect, useMemo, useRef, useState } from "react";
import { Recipe, RecipePrivilege } from "../model";
import { useStore } from "../state/store";
import { effectiveRecipeName, findRecipe, PRIVILEGE_LABEL, TRUST_META } from "../lib/recipes";

interface Props {
  /** The card's dial selection; null inherits the project default. */
  value: string | null;
  projectId: string | null;
  projectDefault: string | null;
  onChange: (recipe: string | null) => void;
  /** Compact framing for the create modal. */
  compact?: boolean;
}

export function RecipeDial({ value, projectId, projectDefault, onChange, compact }: Props) {
  const store = useStore();
  const [open, setOpen] = useState(false);
  // A privileged recipe selection pending consent review (the summary panel).
  const [consenting, setConsenting] = useState<Recipe | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);

  const effectiveName = effectiveRecipeName(value, projectDefault);
  const effective = findRecipe(store.recipes, effectiveName);
  const inherited = value === null;

  // Esc + click-away close the popover.
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
        setConsenting(null);
      }
    };
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) {
        setOpen(false);
        setConsenting(null);
      }
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open]);

  const grantsNeeded = (r: Recipe) => !store.isRecipeGranted(r.name, projectId);

  const pick = (r: Recipe) => {
    if (r.trust === "privileged" && grantsNeeded(r)) {
      // Show exactly what is elevated before the recipe can govern this card.
      setConsenting(r);
      return;
    }
    onChange(r.name);
    setOpen(false);
  };

  const groups = useMemo(() => {
    const bundled = store.recipes.filter((r) => r.scope === "bundled");
    const user = store.recipes.filter((r) => r.scope === "user");
    const project = store.recipes.filter((r) => r.scope === "project");
    return [
      { label: "Bundled", items: bundled },
      { label: "Yours", items: user },
      { label: "This project", items: project },
    ].filter((g) => g.items.length > 0);
  }, [store.recipes]);

  return (
    <div className={`rd${compact ? " is-compact" : ""}`} ref={rootRef}>
      <button
        type="button"
        className={`rd-button${open ? " is-open" : ""}`}
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
        title="Process dial: the flow recipe governing this card"
      >
        <DialGlyph />
        <span className="rd-name">{effectiveName}</span>
        {inherited ? <span className="rd-inherited">default</span> : null}
        {effective?.trust === "privileged" ? <span className="rd-priv-chip">privileged</span> : null}
        <svg className="rd-caret" width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden>
          <path d="M2 3.5l3 3 3-3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>

      {open ? (
        <div className="rd-pop" role="listbox" aria-label="Flow recipe">
          {consenting ? (
            <ConsentSummary
              recipe={consenting}
              projectId={projectId}
              onGrant={() => {
                if (projectId) {
                  store.flash(`Granted "${consenting.name}" for this project.`);
                  grantLocally(consenting, projectId);
                }
                onChange(consenting.name);
                setConsenting(null);
                setOpen(false);
              }}
              onDefer={() => {
                // Select without granting: the dispatch gate re-prompts when it runs.
                onChange(consenting.name);
                setConsenting(null);
                setOpen(false);
              }}
              onBack={() => setConsenting(null)}
            />
          ) : (
            <>
              <button
                type="button"
                className={`rd-row is-inherit${inherited ? " is-current" : ""}`}
                onClick={() => {
                  onChange(null);
                  setOpen(false);
                }}
              >
                <span className="rd-row-name">Project default</span>
                <span className="rd-row-desc">
                  Follow the project's dial ({projectDefault ?? "standard"}).
                </span>
              </button>
              {groups.map((g) => (
                <div key={g.label} className="rd-group">
                  <div className="rd-group-label">{g.label}</div>
                  {g.items.map((r) => (
                    <RecipeRow
                      key={r.name}
                      recipe={r}
                      current={!inherited && r.name === value}
                      needsConsent={r.trust === "privileged" && grantsNeeded(r)}
                      onPick={() => pick(r)}
                    />
                  ))}
                </div>
              ))}
            </>
          )}
        </div>
      ) : null}
    </div>
  );

  // Record the grant through the store's persistence (project::recipe::hash).
  function grantLocally(recipe: Recipe, pid: string) {
    // The store owns grant persistence via the pendingGrant path; for a dial-time
    // grant we reuse the same key set through a tiny shim on the store API.
    store.recordRecipeGrant(pid, recipe.name, recipe.contentHash);
  }
}

function RecipeRow({
  recipe,
  current,
  needsConsent,
  onPick,
}: {
  recipe: Recipe;
  current: boolean;
  needsConsent: boolean;
  onPick: () => void;
}) {
  // Defensive: an unexpected/missing trust tier (e.g. a raw daemon row that slipped
  // normalization) must never throw and blank the app - fall back to the standard tier.
  const trust = TRUST_META[recipe.trust] ?? TRUST_META.standard;
  return (
    <button
      type="button"
      className={`rd-row${current ? " is-current" : ""}`}
      role="option"
      aria-selected={current}
      onClick={onPick}
    >
      <span className="rd-row-head">
        <span className="rd-row-name">{recipe.name}</span>
        {recipe.investigation ? <span className="rd-tag is-invest">investigation</span> : null}
        <span className={`rd-tag trust-${trust.tone}`}>
          {trust.label}
          {needsConsent ? " · grant needed" : ""}
        </span>
      </span>
      <span className="rd-row-desc">{recipe.description}</span>
      <span className="rd-stages">
        {(recipe.stageLines ?? []).map((s) => (
          <span key={`${recipe.name}-${s.stage}`} className="rd-stage">
            <span className="rd-stage-name">{s.stage}</span>
            <span className="rd-stage-note">{s.note}</span>
          </span>
        ))}
      </span>
      {recipe.source !== "bundled" ? <span className="rd-source">{recipe.source}</span> : null}
    </button>
  );
}

/**
 * The consent summary for a privileged recipe: lists exactly what is elevated
 * (security.md), verbatim details included. Shared by the dial popover and the
 * dispatch-time grant modal.
 */
export function ConsentSummary({
  recipe,
  projectId,
  onGrant,
  onDefer,
  deferLabel = "Select, decide at dispatch",
  onBack,
}: {
  recipe: Pick<Recipe, "name" | "privileges">;
  projectId: string | null;
  onGrant: () => void;
  onDefer?: () => void;
  deferLabel?: string;
  onBack?: () => void;
}) {
  return (
    <div className="rd-consent">
      <div className="rd-consent-head">
        {onBack ? (
          <button type="button" className="rd-back" onClick={onBack} aria-label="Back to the recipe list">
            <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden>
              <path d="M9.5 3.5L5 8l4.5 4.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </button>
        ) : null}
        <span className="rd-consent-title">
          &ldquo;{recipe.name}&rdquo; is a privileged recipe
        </span>
      </div>
      <p className="rd-consent-blurb">
        It elevates what a session may do. The grant is per project and re-confirmed if the
        recipe file changes.
      </p>
      <ul className="rd-priv-list">
        {(recipe.privileges ?? []).map((p, i) => (
          <PrivilegeLine key={i} privilege={p} />
        ))}
      </ul>
      <div className="rd-consent-actions">
        {projectId ? (
          <button type="button" className="btn-primary btn-sm" onClick={onGrant}>
            Grant for this project
          </button>
        ) : (
          <span className="rd-consent-note">Pick a project first; the grant is per project.</span>
        )}
        {onDefer ? (
          <button type="button" className="btn-ghost btn-sm" onClick={onDefer}>
            {deferLabel}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function PrivilegeLine({ privilege }: { privilege: RecipePrivilege }) {
  return (
    <li className="rd-priv">
      <span className="rd-priv-kind">{PRIVILEGE_LABEL[privilege.kind] ?? "Elevated capability"}</span>
      <code className="rd-priv-detail">{privilege.detail}</code>
    </li>
  );
}

function DialGlyph() {
  // A rotary dial - the process dial made literal.
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <circle cx="8" cy="8" r="5.6" stroke="currentColor" strokeWidth="1.3" />
      <path d="M8 8L11 5.4" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
      <path d="M8 1.2v1.4M14.8 8h-1.4M8 14.8v-1.4M1.2 8h1.4" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" opacity="0.55" />
    </svg>
  );
}
