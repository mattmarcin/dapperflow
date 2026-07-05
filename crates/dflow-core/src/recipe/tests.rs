//! Recipe engine tests: parsing, precise validation, `extends` inheritance,
//! scoping/resolution, trust classification, and hashing (`recipes.md`, `security.md`).

use std::fs;
use std::path::PathBuf;

use super::*;

/// A unique temp dir for a scope-directory test.
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("dflow-recipe-{tag}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ---- bundled set ----

#[test]
fn every_bundled_recipe_parses() {
    for (hint, text) in bundled_recipe_sources() {
        Recipe::parse(hint, text).unwrap_or_else(|e| panic!("bundled recipe {hint} invalid: {e}"));
    }
    assert_eq!(bundled_recipe_sources().len(), 5);
}

#[test]
fn bundled_dials_are_standard_tier() {
    let catalog = RecipeCatalog::build(None, None);
    for name in ["presto", "standard", "deep", "audit", "audit-deep"] {
        let resolved = catalog.resolve(name).unwrap_or_else(|e| panic!("resolve {name}: {e}"));
        assert_eq!(
            resolved.trust_tier,
            TrustTier::Standard,
            "bundled recipe {name} must be standard tier (proves the layer has no special powers)"
        );
        assert!(resolved.recipe.elevations().is_empty(), "{name} should declare no elevations");
    }
}

#[test]
fn standard_recipe_shape() {
    let catalog = RecipeCatalog::build(None, None);
    let r = catalog.resolve("standard").unwrap().recipe;
    assert_eq!(r.stages, vec![Stage::Plan, Stage::Implement, Stage::Verify, Stage::Ship]);
    let plan = r.plan.as_ref().unwrap();
    assert_eq!(plan.mode, PlanMode::Artifact);
    assert_eq!(plan.approval, Approval::Required);
    assert_eq!(r.worktree_strategy(), WorktreeStrategy::Pooled);
    // M2 interim ship behavior (roadmap M2): checks_only, not full.
    assert_eq!(r.gate(), Some(GateStrictness::ChecksOnly));
    assert_eq!(r.ship.as_ref().unwrap().target, ShipTarget::Pr);
    // Body guidance is stage-tagged and injectable.
    assert!(r.guidance_for(Stage::Plan).unwrap().contains("one screen"));
    assert!(r.guidance_for(Stage::Implement).is_some());
}

#[test]
fn presto_has_no_plan_stage() {
    let catalog = RecipeCatalog::build(None, None);
    let r = catalog.resolve("presto").unwrap().recipe;
    assert!(!r.has_stage(Stage::Plan));
    assert_eq!(r.stages, vec![Stage::Implement, Stage::Verify, Stage::Ship]);
}

#[test]
fn audit_is_investigation_shaped_with_budgets() {
    let catalog = RecipeCatalog::build(None, None);
    let r = catalog.resolve("audit").unwrap().recipe;
    assert_eq!(r.stages, vec![Stage::Implement]);
    assert!(!r.has_stage(Stage::Ship), "audit ships nothing, structurally");
    assert!(r.verify.is_none(), "audit has no verify stage");
    let budgets = r.budgets.unwrap();
    assert_eq!(budgets.cards, Some(10));
    assert_eq!(budgets.notes, Some(6));
    // Shipless recipe with effectively no gate stays standard tier.
    assert_eq!(r.trust_tier(), TrustTier::Standard);
}

// ---- inheritance ----

#[test]
fn audit_deep_inherits_audit_and_raises_budgets() {
    let catalog = RecipeCatalog::build(None, None);
    let base = catalog.resolve("audit").unwrap().recipe;
    let deep = catalog.resolve("audit-deep").unwrap().recipe;
    // extends is cleared after resolution.
    assert!(deep.extends.is_none());
    // Inherited from audit: the implement-only stage list and pooled worktree.
    assert_eq!(deep.stages, base.stages);
    assert_eq!(deep.worktree_strategy(), WorktreeStrategy::Pooled);
    // Overridden: raised budgets and deeper guidance.
    assert_eq!(deep.budgets.unwrap().cards, Some(25));
    assert_eq!(deep.budgets.unwrap().notes, Some(12));
    let deep_guidance = deep.guidance_for(Stage::Implement).unwrap();
    assert!(deep_guidance.contains("deep sweep"), "audit-deep guidance should be its own");
    assert_ne!(deep_guidance, base.guidance_for(Stage::Implement).unwrap());
}

#[test]
fn shallow_merge_replaces_child_blocks_wholesale() {
    // A child that provides its own implement block replaces the parent's entirely.
    let parent = Recipe::parse(
        "base",
        "---\nname: base\nversion: 1\nstages: [implement]\nimplement:\n  harness: claude\n  worktree: pooled\n---\n## implement\nparent guidance.\n",
    )
    .unwrap();
    let child = Recipe::parse(
        "child",
        "---\nname: child\nversion: 2\nextends: base\nstages: [implement]\nimplement:\n  worktree: fresh\n---\n",
    )
    .unwrap();
    let merged = child.merge_over(&parent);
    assert_eq!(merged.name, "child");
    assert_eq!(merged.version, 2);
    assert!(merged.extends.is_none());
    // The child's implement block wins wholesale: harness reverts to the default sentinel.
    assert_eq!(merged.worktree_strategy(), WorktreeStrategy::Fresh);
    assert_eq!(merged.harness_axis(), None, "shallow merge does not deep-merge the block");
    // The child kept no guidance, so the parent's is inherited.
    assert!(merged.guidance_for(Stage::Implement).unwrap().contains("parent guidance"));
}

// ---- scoping / resolution ----

#[test]
fn project_scope_shadows_bundled_and_reports_it() {
    let dir = temp_dir("project-shadow");
    // A project override of `standard` that only tweaks the worktree strategy.
    fs::write(
        dir.join("standard.md"),
        "---\nname: standard\nversion: 9\nextends: standard\nstages: [plan, implement, verify, ship]\nimplement:\n  worktree: fresh\n---\n",
    )
    .unwrap();
    let catalog = RecipeCatalog::build(None, Some(&dir));
    let resolved = catalog.resolve("standard").unwrap();
    assert_eq!(resolved.scope, RecipeScope::Project);
    assert_eq!(resolved.recipe.version, 9);
    // extends: standard resolved to the bundled standard, then the fresh override applied.
    assert_eq!(resolved.recipe.worktree_strategy(), WorktreeStrategy::Fresh);
    // The bundled standard is shadowed, and the catalog reports it.
    let shadowed = catalog.shadowed_scopes("standard");
    assert_eq!(shadowed, vec![RecipeScope::Bundled]);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn user_scope_is_between_bundled_and_project() {
    let user = temp_dir("user-scope");
    fs::write(
        user.join("myflow.md"),
        "---\nname: myflow\nversion: 1\nstages: [implement, verify]\nverify:\n  gate: checks_only\n---\n## implement\ngo.\n",
    )
    .unwrap();
    let catalog = RecipeCatalog::build(Some(&user), None);
    let resolved = catalog.resolve("myflow").unwrap();
    assert_eq!(resolved.scope, RecipeScope::User);
    assert!(catalog.contains("presto"), "bundled recipes remain visible alongside user recipes");
    fs::remove_dir_all(&user).ok();
}

#[test]
fn resolve_unknown_recipe_errors() {
    let catalog = RecipeCatalog::build(None, None);
    assert_eq!(catalog.resolve("nope").unwrap_err(), ResolveError::NotFound("nope".into()));
}

#[test]
fn extends_missing_parent_errors() {
    let dir = temp_dir("orphan");
    fs::write(
        dir.join("orphan.md"),
        "---\nname: orphan\nversion: 1\nextends: ghost\nstages: [implement]\n---\n",
    )
    .unwrap();
    let catalog = RecipeCatalog::build(None, Some(&dir));
    let err = catalog.resolve("orphan").unwrap_err();
    assert!(matches!(err, ResolveError::ExtendsNotFound { .. }), "got {err:?}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn extends_cycle_is_detected() {
    let dir = temp_dir("cycle");
    fs::write(dir.join("a.md"), "---\nname: a\nversion: 1\nextends: b\nstages: [implement]\n---\n").unwrap();
    fs::write(dir.join("b.md"), "---\nname: b\nversion: 1\nextends: a\nstages: [implement]\n---\n").unwrap();
    let catalog = RecipeCatalog::build(None, Some(&dir));
    assert!(matches!(catalog.resolve("a"), Err(ResolveError::Cycle(_))));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn malformed_recipe_is_invalid_not_fatal() {
    let dir = temp_dir("invalid");
    fs::write(dir.join("broken.md"), "no front matter here\n").unwrap();
    fs::write(
        dir.join("good.md"),
        "---\nname: good\nversion: 1\nstages: [implement]\n---\n## implement\nok.\n",
    )
    .unwrap();
    let catalog = RecipeCatalog::build(None, Some(&dir));
    // The good one resolves; the broken one is surfaced but not resolvable.
    assert!(catalog.resolve("good").is_ok());
    assert_eq!(catalog.invalid().len(), 1);
    assert_eq!(catalog.invalid()[0].name_hint, "broken");
    fs::remove_dir_all(&dir).ok();
}

// ---- trust classification table ----

fn parse(text: &str) -> Recipe {
    Recipe::parse("t", text).unwrap()
}

#[test]
fn trust_tier_table() {
    // mcp mount -> privileged.
    let mcp = parse(
        "---\nname: t\nversion: 1\nstages: [implement]\nmcp:\n  - name: c7\n    command: \"npx c7\"\n    stages: [implement]\n---\n",
    );
    assert_eq!(mcp.trust_tier(), TrustTier::Privileged);
    assert!(mcp.elevations().iter().any(|e| matches!(e, Elevation::McpMount { .. })));

    // worktree in_place -> privileged.
    let in_place = parse("---\nname: t\nversion: 1\nstages: [implement]\nimplement:\n  worktree: in_place\n---\n");
    assert_eq!(in_place.trust_tier(), TrustTier::Privileged);
    assert!(in_place.elevations().contains(&Elevation::WorktreeInPlace));

    // gate none WITH a ship stage -> privileged.
    let gate_ship = parse(
        "---\nname: t\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: none\nship:\n  target: pr\n---\n",
    );
    assert_eq!(gate_ship.trust_tier(), TrustTier::Privileged);
    assert!(gate_ship.elevations().contains(&Elevation::GateDisabledWithShip));

    // gate none WITHOUT a ship stage -> standard (the audit case clarification).
    let gate_no_ship =
        parse("---\nname: t\nversion: 1\nstages: [implement, verify]\nverify:\n  gate: none\n---\n");
    assert_eq!(gate_no_ship.trust_tier(), TrustTier::Standard);

    // ship local_merge -> privileged.
    let local_merge = parse(
        "---\nname: t\nversion: 1\nstages: [implement, verify, ship]\nverify:\n  gate: checks_only\nship:\n  target: local_merge\n---\n",
    );
    assert_eq!(local_merge.trust_tier(), TrustTier::Privileged);
    assert!(local_merge.elevations().contains(&Elevation::ShipLocalMerge));
}

// ---- validation precision ----

#[test]
fn validation_errors_are_precise() {
    // Missing front matter.
    assert!(Recipe::parse("t", "just a body\n").is_err());
    // Missing version.
    assert!(Recipe::parse("t", "---\nname: t\nstages: [implement]\n---\n").unwrap_err().message.contains("version"));
    // Missing stages.
    assert!(Recipe::parse("t", "---\nname: t\nversion: 1\n---\n").unwrap_err().message.contains("stages"));
    // Unknown stage.
    let e = Recipe::parse("t", "---\nname: t\nversion: 1\nstages: [design]\n---\n").unwrap_err();
    assert!(e.message.contains("unknown stage 'design'"));
    // Duplicate stage.
    let e = Recipe::parse("t", "---\nname: t\nversion: 1\nstages: [implement, implement]\n---\n").unwrap_err();
    assert!(e.message.contains("more than once"));
    // Out-of-order stages.
    let e = Recipe::parse("t", "---\nname: t\nversion: 1\nstages: [ship, implement]\n---\n").unwrap_err();
    assert!(e.message.contains("pipeline order"));
    // Bad enum value with the field named.
    let e = Recipe::parse(
        "t",
        "---\nname: t\nversion: 1\nstages: [implement]\nimplement:\n  worktree: bogus\n---\n",
    )
    .unwrap_err();
    assert!(e.message.contains("implement.worktree"), "got: {e}");
    // Bad gate value.
    let e = Recipe::parse(
        "t",
        "---\nname: t\nversion: 1\nstages: [verify]\nverify:\n  gate: sorta\n---\n",
    )
    .unwrap_err();
    assert!(e.message.contains("verify.gate"));
    // Bad budget value.
    let e = Recipe::parse(
        "t",
        "---\nname: t\nversion: 1\nstages: [implement]\nbudgets:\n  cards: lots\n---\n",
    )
    .unwrap_err();
    assert!(e.message.contains("budgets.cards"));
}

#[test]
fn front_matter_line_numbers_surface() {
    // A YAML-level error carries the offending line from the subset parser.
    let e = Recipe::parse("t", "---\nname: t\nversion: 1\nstages: [implement]\n\tbad: tab\n---\n").unwrap_err();
    assert_eq!(e.line, Some(5), "tab error should point at the source line: {e:?}");
}

// ---- hashing ----

#[test]
fn content_hash_is_stable_and_change_sensitive() {
    let a = content_hash("---\nname: t\nversion: 1\nstages: [implement]\n---\n");
    let b = content_hash("---\nname: t\nversion: 1\nstages: [implement]\n---\n");
    assert_eq!(a, b, "same text hashes identically (grant stays valid)");
    let c = content_hash("---\nname: t\nversion: 2\nstages: [implement]\n---\n");
    assert_ne!(a, c, "a file edit changes the hash (grant invalidates)");
    assert_eq!(a.len(), 40, "sha1 hex is 40 chars");
}

#[test]
fn default_axes_map_to_none() {
    let r = parse(
        "---\nname: t\nversion: 1\nstages: [implement]\nimplement:\n  harness: default\n  model: default\n  effort: high\n  worktree: pooled\n---\n",
    );
    assert_eq!(r.harness_axis(), None);
    assert_eq!(r.model_axis(), None);
    assert_eq!(r.effort_axis(), Some("high"));
}
