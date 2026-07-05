//! The bundled recipe set, compiled into the binary from `recipes/` at the repo root.
//!
//! The bundled dials (`presto`, `standard`, `deep`) and the onboarding-audit recipes
//! (`audit`, `audit-deep`) ship as real markdown files with no special powers, which is
//! the proof that the recipe layer is real (`recipes.md` / Goals). They are `include_str!`d
//! so the daemon always ships a valid, self-consistent set; a unit test parses every one
//! at build-confidence time.

/// `(name, markdown)` for every bundled recipe. The name matches the file stem, but the
/// authoritative name is the front matter's `name` field once parsed.
pub const BUNDLED_RECIPES: &[(&str, &str)] = &[
    ("presto", include_str!("../../../../recipes/presto.md")),
    ("standard", include_str!("../../../../recipes/standard.md")),
    ("deep", include_str!("../../../../recipes/deep.md")),
    ("audit", include_str!("../../../../recipes/audit.md")),
    ("audit-deep", include_str!("../../../../recipes/audit-deep.md")),
];

/// The bundled recipe sources as `(name_hint, text)` pairs.
pub fn bundled_recipe_sources() -> &'static [(&'static str, &'static str)] {
    BUNDLED_RECIPES
}
